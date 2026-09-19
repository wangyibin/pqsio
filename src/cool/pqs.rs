//! Project only contact coordinates and quality, without materializing IDs or strands.
use super::*;
use crate::{Codes, Numbers};
use polars::prelude::{ParallelStrategy, ParquetReader, SerReader};
use std::fs::File;
use std::sync::Arc;
use std::sync::{atomic::AtomicBool, mpsc, Mutex};
use std::thread;

struct Job {
    path: PathBuf,
    reader: ParquetReader<File>,
}

// A row group normally decodes to one non-null chunk per column. Borrow its
// contiguous values once, avoiding a ChunkedArray::get search for every cell.
// Unusual or nullable columns retain the shared reader's checked fallback.
enum NumberView<'a> {
    U8(&'a [u8]),
    U32(&'a [u32]),
    U64(&'a [u64]),
    General(&'a Numbers),
}
impl<'a> NumberView<'a> {
    fn new(numbers: &'a Numbers) -> Self {
        let view = match numbers {
            Numbers::U8(c) => c.cont_slice().ok().map(Self::U8),
            Numbers::U32(c) => c.cont_slice().ok().map(Self::U32),
            Numbers::U64(c) => c.cont_slice().ok().map(Self::U64),
        };
        view.unwrap_or(Self::General(numbers))
    }
    #[inline]
    fn at(&self, i: usize) -> Result<u64> {
        match self {
            Self::U8(c) => c.get(i).copied().map(u64::from),
            Self::U32(c) => c.get(i).copied().map(u64::from),
            Self::U64(c) => c.get(i).copied(),
            Self::General(c) => return c.at(i),
        }
        .context("missing integer in PQS")
    }
}

enum CodeView<'a> {
    Local(&'a [u32], &'a [Option<usize>]),
    Plain(&'a [usize]),
    General(&'a Codes<usize>),
}
impl<'a> CodeView<'a> {
    fn new(codes: &'a Codes<usize>) -> Self {
        match codes {
            Codes::Local { indices, values } => indices
                .cont_slice()
                .map(|indices| Self::Local(indices, values))
                .unwrap_or(Self::General(codes)),
            Codes::Plain(values) => Self::Plain(values),
        }
    }
    #[inline]
    fn at(&self, i: usize) -> Result<usize> {
        match self {
            Self::Local(indices, values) => indices
                .get(i)
                .and_then(|&code| values.get(code as usize))
                .copied()
                .flatten()
                .context("invalid contig in categorical column"),
            Self::Plain(values) => values.get(i).copied().context("missing categorical value"),
            Self::General(codes) => codes.at(i),
        }
    }
}

fn jobs(
    files: impl Iterator<Item = PathBuf>,
    mut submit: impl FnMut(Job) -> Result<()>,
) -> Result<()> {
    for path in files {
        let mut footer = ParquetReader::new(File::open(&path)?);
        let metadata = footer
            .get_metadata()
            .with_context(|| format!("PQS shard {}", path.display()))?;
        // Clone the full row-group list once, not once per job (quadratic work).
        let mut template = metadata.as_ref().clone();
        template.row_groups.clear();
        for group in &metadata.row_groups {
            if group.num_rows() == 0 {
                continue;
            }
            let mut one = template.clone();
            one.num_rows = group.num_rows();
            one.row_groups = vec![group.clone()];
            // Independent file handles, so concurrent decoders never share a seek position.
            let mut reader = ParquetReader::new(File::open(&path)?)
                .with_columns(Some(
                    ["chrom1", "pos1", "chrom2", "pos2", "mapq"]
                        .map(String::from)
                        .to_vec(),
                ))
                .read_parallel(ParallelStrategy::None);
            reader.set_metadata(Arc::new(one));
            submit(Job {
                path: path.clone(),
                reader,
            })?;
        }
    }
    Ok(())
}

fn process(
    job: Job,
    refs: &References,
    options: &CoolOptions,
    mut emit: impl FnMut(Vec<sort::Pixel>) -> Result<()>,
) -> Result<Statistics> {
    (|| -> Result<Statistics> {
        let frame = job.reader.finish()?;
        let chromosome = |name| {
            Codes::new(frame.column(name)?, |value| {
                refs.ids
                    .get(value)
                    .copied()
                    .context("unknown contig in shard")
            })
        };
        let a = chromosome("chrom1")?;
        let b = chromosome("chrom2")?;
        let x = Numbers::new(frame.column("pos1")?)?;
        let y = Numbers::new(frame.column("pos2")?)?;
        let quality = Numbers::new(frame.column("mapq")?)?;
        let (a, b) = (CodeView::new(&a), CodeView::new(&b));
        let (x, y, quality) = (
            NumberView::new(&x),
            NumberView::new(&y),
            NumberView::new(&quality),
        );
        let mut stats = Statistics::default();
        let mut rows = Vec::new();
        for i in 0..frame.height() {
            // Validate required column values even when MAPQ removes a record.
            let (a, b, x, y) = (a.at(i)?, b.at(i)?, x.at(i)?, y.at(i)?);
            let mapq = u8::try_from(quality.at(i)?).context("MAPQ exceeds UInt8")?;
            stats.input_records += 1;
            if mapq < options.min_mapq {
                stats.skipped_mapq += 1;
                continue;
            }
            let (one, two) = (invalid(refs.bin(a, x))?, invalid(refs.bin(b, y))?);
            rows.push(sort::Pixel {
                bin1: one.min(two),
                bin2: one.max(two),
                count: 1,
            });
            stats.contacts += 1;
            if rows.len() == options.batch_rows {
                emit(std::mem::take(&mut rows))?;
            }
        }
        if !rows.is_empty() {
            emit(rows)?;
        }
        Ok(stats)
    })()
    .with_context(|| format!("PQS shard {}", job.path.display()))
}

fn add_stats(total: &mut Statistics, part: Statistics) {
    total.input_records += part.input_records;
    total.skipped_mapq += part.skipped_mapq;
    total.contacts += part.contacts;
}

pub(super) fn load(
    files: impl Iterator<Item = PathBuf> + Send,
    refs: &References,
    options: &CoolOptions,
    stats: &mut Statistics,
    sorter: &mut sort::Sorter,
) -> Result<()> {
    if options.threads == 1 {
        return jobs(files, |job| {
            add_stats(
                stats,
                process(job, refs, options, |rows| sorter.extend(rows))?,
            );
            Ok(())
        });
    }
    enum Message {
        Pixels(Vec<sort::Pixel>),
        Stats(Statistics),
        Error(anyhow::Error),
    }
    thread::scope(|scope| -> Result<()> {
        let (send_job, receive_job) = mpsc::sync_channel::<Job>(options.threads);
        let receive_job = Arc::new(Mutex::new(receive_job));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (send, receive) = mpsc::sync_channel(options.threads);
        for index in 0..options.threads {
            let (jobs, output, cancelled) = (
                Arc::clone(&receive_job),
                send.clone(),
                Arc::clone(&cancelled),
            );
            thread::Builder::new()
                .name(format!("cool-read-{index}"))
                .spawn_scoped(scope, move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                            while !cancelled.load(Ordering::Relaxed) {
                                let job = jobs
                                    .lock()
                                    .map_err(|_| anyhow::anyhow!("PQS job queue poisoned"))?
                                    .recv();
                                let Ok(job) = job else {
                                    break;
                                };
                                let stats = process(job, refs, options, |rows| {
                                    output
                                        .send(Message::Pixels(rows))
                                        .context("Cooler conversion cancelled")
                                })?;
                                output
                                    .send(Message::Stats(stats))
                                    .context("Cooler conversion cancelled")?;
                            }
                            Ok(())
                        }))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("PQS decoder worker panicked")));
                    if let Err(error) = result {
                        cancelled.store(true, Ordering::Relaxed);
                        let _ = output.send(Message::Error(error));
                    }
                })
                .context("cannot start PQS decoder worker")?;
        }
        // Only workers retain the job receiver. Dropping the result receiver on
        // error releases blocked producers, then all scoped threads are joined.
        drop(receive_job);
        let output = send.clone();
        thread::Builder::new()
            .name("cool-footers".into())
            .spawn_scoped(scope, move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    jobs(files, |job| {
                        ensure!(
                            !cancelled.load(Ordering::Relaxed),
                            "Cooler conversion cancelled"
                        );
                        send_job.send(job).context("Cooler conversion cancelled")
                    })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("PQS footer worker panicked")));
                if let Err(error) = result {
                    // A decoder that cancelled us sends its own contextual error.
                    // Do not race it with a generic "conversion cancelled" message.
                    if !cancelled.load(Ordering::Relaxed) {
                        let _ = output.send(Message::Error(error));
                    }
                }
            })
            .context("cannot start PQS footer worker")?;
        drop(send);
        for message in receive {
            match message {
                Message::Pixels(rows) => sorter.extend(rows)?,
                Message::Stats(part) => add_stats(stats, part),
                Message::Error(error) => return Err(error),
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use polars::prelude::{NewChunkedArray, UInt32Chunked, UInt64Chunked, UInt8Chunked};

    #[test]
    fn column_views_preserve_null_chunk_and_dictionary_checks() -> Result<()> {
        let numbers = [
            Numbers::U8(UInt8Chunked::from_slice("x".into(), &[1, 255])),
            Numbers::U32(UInt32Chunked::from_slice("x".into(), &[1, u32::MAX])),
            Numbers::U64(UInt64Chunked::from_slice("x".into(), &[1, 1u64 << 40])),
        ];
        for (numbers, last) in numbers.iter().zip([255, u32::MAX as u64, 1u64 << 40]) {
            let view = NumberView::new(numbers);
            assert!(!matches!(view, NumberView::General(_)));
            assert_eq!(view.at(0)?, 1);
            assert_eq!(view.at(1)?, last);
            assert!(view.at(2).is_err());
        }
        let nullable = Numbers::U32(UInt32Chunked::from_iter_options(
            "x".into(),
            [Some(7), None].into_iter(),
        ));
        let view = NumberView::new(&nullable);
        assert!(matches!(view, NumberView::General(_)));
        assert_eq!(view.at(0)?, 7);
        assert!(view.at(1).is_err());
        let mut chunks = UInt32Chunked::from_slice("x".into(), &[1, 2]);
        chunks.append(&UInt32Chunked::from_slice("x".into(), &[3]))?;
        let chunks = Numbers::U32(chunks);
        let view = NumberView::new(&chunks);
        assert!(matches!(view, NumberView::General(_)));
        assert_eq!(view.at(2)?, 3);

        let codes = Codes::Local {
            indices: UInt32Chunked::from_slice("c".into(), &[0, 1]),
            values: vec![Some(9), None],
        };
        let view = CodeView::new(&codes);
        assert!(matches!(view, CodeView::Local(_, _)));
        assert_eq!(view.at(0)?, 9);
        assert!(view.at(1).is_err());
        let nullable = Codes::Local {
            indices: UInt32Chunked::from_iter_options("c".into(), [Some(0), None].into_iter()),
            values: vec![Some(9)],
        };
        let view = CodeView::new(&nullable);
        assert!(matches!(view, CodeView::General(_)));
        assert_eq!(view.at(0)?, 9);
        assert!(view.at(1).is_err());
        Ok(())
    }

    #[test]
    fn parquet_jobs_decode_only_five_columns() -> Result<()> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&parent)?;
        let root = Guard::scratch(&parent)?;
        let input = root.path.join("pairs");
        let mut writer = crate::Writer::create(
            &input,
            Kind::Pairs,
            vec![Contig {
                name: "a".into(),
                length: 100,
            }],
            1,
        )?;
        writer.write_pairs(&[crate::Pair {
            read_id: "large-id".repeat(1024),
            chrom1: 0,
            pos1: 1,
            chrom2: 0,
            pos2: 100,
            strand1: b'+',
            strand2: b'-',
            mapq: 30,
        }])?;
        writer.finish()?;
        let reader = Reader::open(&input, 0)?;
        let mut rows = 0;
        jobs(reader.files, |job| {
            let frame = job.reader.finish()?;
            let mut names: Vec<_> = frame
                .get_column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            names.sort();
            assert_eq!(names, ["chrom1", "chrom2", "mapq", "pos1", "pos2"]);
            rows += frame.height();
            Ok(())
        })?;
        assert_eq!(rows, 1);
        Ok(())
    }
}
