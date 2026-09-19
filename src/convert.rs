//! Streaming concat PQS to pairs PQS conversion.
use crate::*;
use crate::metadata::{obj, Value};
use std::ops::Range;
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug)]
pub struct ConvertOptions {
    /// Conversion worker limit; when > 1 also starts this many encoding workers.
    /// Must be positive; does not cap Polars' internal thread pool.
    pub threads: usize,
    pub chunk_size: usize,
    pub batch_rows: usize,
    pub min_mapq: u8,
    pub min_order: usize,
    /// Exclusive upper bound, applied after MAPQ filtering.
    pub max_order: usize,
}
impl Default for ConvertOptions {
    fn default() -> Self {
        Self { threads: 1, chunk_size: 1_000_000, batch_rows: 65_536, min_mapq: 0,
            min_order: 2, max_order: usize::MAX }
    }
}

#[derive(Clone, Debug)]
pub struct ConvertResult {
    pub output: PathBuf,
    pub counts: Counts,
    pub output_reads: u64,
}
impl ConvertResult {
    pub fn to_json(&self) -> String {
        obj([
            ("output", Value::from(self.output.to_string_lossy().as_ref())),
            ("mode", Value::from("concat2pairs")),
            ("format", Value::from("pairs")),
            ("output_reads", Value::from(self.output_reads)),
            ("counts", obj([
                ("q0_records", Value::from(self.counts.q0_records)),
                ("q1_records", Value::from(self.counts.q1_records)),
            ])),
        ]).json()
    }
}

/// Expand every eligible read into all unordered alignment pairs, ordered by
/// query start. Positions are 1-based reference midpoints; MAPQ is the minimum
/// of both ends. Pair IDs encode logical read ID and sorted alignment indices.
pub fn convert(input: impl AsRef<Path>, output: impl AsRef<Path>, mode: &str,
    options: ConvertOptions) -> Result<ConvertResult> {
    ensure!(mode == "concat2pairs", "unsupported conversion mode: {mode}; expected concat2pairs");
    ensure!(options.min_order >= 2 && options.max_order > options.min_order,
        "require 2 <= min_order < max_order (exclusive)");
    ensure!(options.threads > 0, "threads must be positive");
    // Scan q0 even when MAPQ > 0, preserving logical IDs independently of q1.
    let source = Reader::open(input.as_ref(), 0)?;
    ensure!(source.kind == Kind::Concat, "concat2pairs requires a concat PQS input");
    let mut reader = StreamingReader::from_source(source, 0, ReadOptions {
        batch_rows: options.batch_rows, boundary: ReadBoundary::CompleteReads,
        concat_filter: None,
    })?;
    let cn = read_copy_numbers(input.as_ref())?;
    let mut writer = Writer::create(output.as_ref(), Kind::Pairs,
        reader.contigs().to_vec(), options.chunk_size)?;
    if cn.present { writer.set_copy_numbers(&cn.explicit)?; }
    if options.threads > 1 { writer.parallel_encoding(options.threads)?; }
    let mut pool = if options.threads > 1 {
        Some(ConversionPool::new(options)?)
    } else { None };
    let mut scratch = ExpandScratch::default();
    let mut output_reads = 0u64;
    progress::emit("Expanding and writing pairs", 0, 0);
    while let Some(batch) = reader.next_columns()? {
        let ColumnBatch::Concat(rows) = batch else { unreachable!() };
        let count = if let Some(pool) = &mut pool {
            pool.run(rows, &mut writer)?
        } else {
            expand(&rows, 0..rows.read_offsets.len() - 1, options, &mut scratch,
                |pairs| { writer.write_pairs_columns(pairs.as_view())?; Ok(pairs) })?
        };
        output_reads = output_reads.checked_add(count).context("read count overflow")?;
        progress::emit("Expanding and writing pairs", output_reads, 0);
    }
    progress::emit("Finalizing PQS", output_reads, 0);
    let counts = writer.finish()?;
    Ok(ConvertResult { output: output.as_ref().to_path_buf(), counts, output_reads })
}

enum Message { Pairs(PairColumns), Done(Result<u64>) }
struct ConversionJob {
    rows: Arc<ConcatColumns>,
    reads: Range<usize>,
    reply: mpsc::SyncSender<Message>,
}

/// Persistent workers with one bounded task queue each. One input batch is
/// shared without cloning columns; output streams are consumed in source order.
struct ConversionPool {
    senders: Vec<mpsc::SyncSender<ConversionJob>>,
    workers: Vec<JoinHandle<()>>,
    recycle: Vec<mpsc::SyncSender<PairColumns>>,
    scratch: ExpandScratch,
    options: ConvertOptions,
    failed: bool,
}
impl ConversionPool {
    fn new(options: ConvertOptions) -> Result<Self> {
        ensure!(options.threads > 0, "threads must be positive");
        // Partial startup failure drops senders and joins already-started workers.
        let mut pool = Self { senders: vec![], workers: vec![], recycle: vec![],
            scratch: ExpandScratch::default(), options, failed: false };
        for index in 0..options.threads {
            let (tx, rx) = mpsc::sync_channel::<ConversionJob>(1);
            let (recycle_tx, recycle_rx) = mpsc::sync_channel(1);
            let worker = thread::Builder::new().name(format!("pqs-convert-{index}"))
                .spawn(move || {
                    let mut scratch = ExpandScratch::default();
                    while let Ok(job) = rx.recv() {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            expand(&job.rows, job.reads, options, &mut scratch, |pairs| {
                                job.reply.send(Message::Pairs(pairs))
                                    .map_err(|_| anyhow::anyhow!("conversion cancelled"))?;
                                // Never wait for recycling: ordering/cancellation must
                                // only depend on the bounded forward output stream.
                                Ok(recycle_rx.try_recv().unwrap_or_default())
                            })
                        })).unwrap_or_else(|_| Err(anyhow::anyhow!("conversion worker panicked")));
                        let _ = job.reply.send(Message::Done(result));
                    }
                }).context("cannot start conversion worker")?;
            pool.recycle.push(recycle_tx);
            pool.senders.push(tx);
            pool.workers.push(worker);
        }
        Ok(pool)
    }

    fn run(&mut self, rows: ConcatColumns, writer: &mut Writer) -> Result<u64> {
        ensure!(!self.failed, "conversion pool failed; create a new conversion");
        let result = self.run_inner(rows, writer);
        if result.is_err() { self.failed = true; }
        result
    }

    fn run_inner(&mut self, rows: ConcatColumns, writer: &mut Writer) -> Result<u64> {
        let reads = rows.read_offsets.len() - 1;
        if reads == 0 { return Ok(0); }
        let workers = self.senders.len().min(reads);
        if workers == 1 {
            return expand(&rows, 0..reads, self.options, &mut self.scratch,
                |pairs| { writer.write_pairs_columns(pairs.as_view())?; Ok(pairs) });
        }
        let rows = Arc::new(rows);
        // These receivers must be dropped before pool Drop joins workers on
        // any error, releasing workers blocked on a full output queue.
        let mut receivers = Vec::new();
        let per_worker = reads.div_ceil(workers);
        for (index, first) in (0..reads).step_by(per_worker).enumerate() {
            let last = (first + per_worker).min(reads);
            let (reply, rx) = mpsc::sync_channel(1);
            receivers.push(rx);
            self.senders[index].send(ConversionJob {
                rows: Arc::clone(&rows), reads: first..last, reply,
            }).map_err(|_| anyhow::anyhow!("conversion worker disconnected"))?;
        }
        let mut count = 0u64;
        for (index, rx) in receivers.iter().enumerate() {
            loop {
                match rx.recv().context("conversion worker disconnected")? {
                    Message::Pairs(mut pairs) => {
                        writer.write_pairs_columns(pairs.as_view())?;
                        clear_pairs(&mut pairs);
                        // At most one spare buffer per worker; discard excess
                        // instead of blocking a worker or growing a free list.
                        let _ = self.recycle[index].try_send(pairs);
                    },
                    Message::Done(result) => {
                        count = count.checked_add(result?).context("read count overflow")?;
                        break;
                    }
                }
            }
        }
        Ok(count)
    }
}
impl Drop for ConversionPool {
    fn drop(&mut self) {
        self.senders.clear();
        for worker in self.workers.drain(..) { let _ = worker.join(); }
    }
}

/// Cached decimal representation, valid for all UInt64 IDs and usize indices.
struct Decimal {
    bytes: [u8; 20],
    start: usize,
}
impl Decimal {
    fn new(mut value: u64) -> Self {
        let mut encoded = Self { bytes: [0; 20], start: 20 };
        loop {
            encoded.start -= 1;
            encoded.bytes[encoded.start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 { return encoded; }
        }
    }
    fn bytes(&self) -> &[u8] { &self.bytes[self.start..] }
}

#[derive(Default)]
struct ExpandScratch {
    read: Vec<usize>,
    positions: Vec<u64>,
    indices: Vec<Decimal>,
    prefix: Vec<u8>,
    pairs: PairColumns,
}

fn clear_pairs(pairs: &mut PairColumns) {
    pairs.read_id_offsets.clear();
    pairs.read_id_offsets.push(0);
    pairs.read_id_bytes.clear();
    pairs.chrom1.clear();
    pairs.chrom2.clear();
    pairs.pos1.clear();
    pairs.pos2.clear();
    pairs.strand1.clear();
    pairs.strand2.clear();
    pairs.mapq.clear();
}

fn emit_pairs(pairs: &mut PairColumns,
    emit: &mut impl FnMut(PairColumns) -> Result<PairColumns>) -> Result<()> {
    // Swap with a zero-allocation placeholder; the returned buffer is either
    // the same synchronous buffer, a recycled worker buffer, or a fresh one.
    let mut batch = PairColumns {
        read_id_offsets: vec![], read_id_bytes: vec![],
        chrom1: vec![], chrom2: vec![], pos1: vec![], pos2: vec![],
        strand1: vec![], strand2: vec![], mapq: vec![],
    };
    std::mem::swap(pairs, &mut batch);
    *pairs = emit(batch)?;
    clear_pairs(pairs);
    Ok(())
}

fn expand(rows: &ConcatColumns, reads: Range<usize>, options: ConvertOptions,
    scratch: &mut ExpandScratch,
    mut emit: impl FnMut(PairColumns) -> Result<PairColumns>) -> Result<u64> {
    let mut count = 0u64;
    let ExpandScratch { read, positions, indices, prefix, pairs } = scratch;
    for idx in reads {
        let start = rows.read_offsets[idx] as usize;
        let end = rows.read_offsets[idx + 1] as usize;
        read.clear();
        read.extend((start..end).filter(|&i| rows.mapping_quality[i] >= options.min_mapq));
        if read.len() < options.min_order || read.len() >= options.max_order { continue; }
        // (query start, source row) has the same order as stable query-start
        // sorting, without allocating a stable-sort workspace on every read.
        read.sort_unstable_by_key(|&i| (rows.read_start[i], i));
        positions.clear();
        for &i in read.iter() {
            ensure!(rows.end[i] > rows.start[i], "concat alignment must have end > start");
            positions.push((rows.start[i] + (rows.end[i] - rows.start[i]) / 2)
                .checked_add(1).context("1-based midpoint overflow")?);
        }
        for index in indices.len()..read.len() { indices.push(Decimal::new(index as u64)); }
        prefix.clear();
        prefix.extend_from_slice(Decimal::new(rows.read_idx[read[0]]).bytes());
        prefix.push(b':');
        let read_prefix_len = prefix.len();
        count = count.checked_add(1).context("read count overflow")?;
        for left in 0..read.len() - 1 {
            let a = read[left];
            let chrom1 = rows.chrom[a];
            let pos1 = positions[left];
            let strand1 = rows.strand[a];
            let mapq1 = rows.mapping_quality[a];
            prefix.truncate(read_prefix_len);
            prefix.extend_from_slice(indices[left].bytes());
            prefix.push(b':');
            for right in left + 1..read.len() {
                let b = read[right];
                pairs.read_id_bytes.extend_from_slice(prefix);
                pairs.read_id_bytes.extend_from_slice(indices[right].bytes());
                pairs.read_id_offsets.push(pairs.read_id_bytes.len() as u64);
                pairs.chrom1.push(chrom1);
                pairs.chrom2.push(rows.chrom[b]);
                pairs.pos1.push(pos1);
                pairs.pos2.push(positions[right]);
                pairs.strand1.push(strand1);
                pairs.strand2.push(rows.strand[b]);
                pairs.mapq.push(mapq1.min(rows.mapping_quality[b]));
                if pairs.pos1.len() >= options.batch_rows { emit_pairs(pairs, &mut emit)?; }
            }
        }
    }
    if !pairs.pos1.is_empty() { emit_pairs(pairs, &mut emit)?; }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_encoding_matches_uint64_boundaries() {
        let mut values = vec![0, 1, u32::MAX as u64, u64::MAX];
        for power in 0..20 {
            let value = 10u64.pow(power);
            values.extend([value - 1, value, value + 1]);
        }
        for value in values {
            assert_eq!(Decimal::new(value).bytes(), value.to_string().as_bytes());
        }
    }

    #[test]
    fn cached_ids_and_reused_buffers_preserve_content() -> Result<()> {
        let mut scratch = ExpandScratch::default();
        let options = ConvertOptions { batch_rows: 3, ..Default::default() };
        let mut previous_buffers = None;
        for id in [u64::MAX, 0, 10] {
            let rows = ConcatColumns {
                read_offsets: vec![0, 12], read_idx: vec![id; 12],
                read_start: (0..12).map(|i| (11 - i) / 2).collect(),
                chrom: vec![0; 12], start: (0..12).collect(), end: (1..13).collect(),
                strand: vec![b'+'; 12], mapping_quality: vec![30; 12],
                ..Default::default()
            };
            let mut observed = Vec::new();
            assert_eq!(expand(&rows, 0..1, options, &mut scratch, |pairs| {
                for i in 0..pairs.pos1.len() {
                    observed.push((String::from_utf8(pairs.read_id_bytes[
                        pairs.read_id_offsets[i] as usize..pairs.read_id_offsets[i + 1] as usize
                    ].to_vec())?, pairs.pos1[i], pairs.pos2[i]));
                }
                Ok(pairs)
            })?, 1);
            let sorted = [10u64, 11, 8, 9, 6, 7, 4, 5, 2, 3, 0, 1];
            let mut expected = Vec::new();
            for left in 0..12 {
                for right in left + 1..12 {
                    expected.push((format!("{id}:{left}:{right}"), sorted[left] + 1, sorted[right] + 1));
                }
            }
            assert_eq!(observed, expected);
            assert!(scratch.pairs.pos1.is_empty());
            assert!(scratch.pairs.read_id_bytes.is_empty());
            assert_eq!(scratch.pairs.read_id_offsets, [0]);
            let buffers = (scratch.pairs.pos1.as_ptr(), scratch.pairs.read_id_bytes.as_ptr(),
                           scratch.read.as_ptr(), scratch.positions.as_ptr(), scratch.indices.as_ptr());
            if let Some(previous) = previous_buffers { assert_eq!(buffers, previous); }
            previous_buffers = Some(buffers);
        }
        Ok(())
    }

    #[test]
    fn conversion_workers_are_reused_across_batches() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output")
            .join(format!("convert-pool-reuse-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
        }
        let _cleanup = Cleanup(root.clone());
        let output = root.join("pairs");
        let mut writer = Writer::create(&output, Kind::Pairs,
            vec![Contig { name: "a".into(), length: 100 }], 7)?;
        writer.parallel_encoding(2)?;
        let mut pool = ConversionPool::new(ConvertOptions {
            threads: 2, batch_rows: 1, ..Default::default()
        })?;
        let ids: Vec<_> = pool.workers.iter().map(|w| w.thread().id()).collect();
        let mut expected = Vec::new();
        for batch in 0..20 {
            let rows = ConcatColumns {
                read_offsets: vec![0, 2, 4],
                read_idx: vec![batch * 2, batch * 2, batch * 2 + 1, batch * 2 + 1],
                read_start: vec![0, 1, 0, 1], chrom: vec![0; 4],
                start: vec![0, 1, 0, 1], end: vec![1, 2, 1, 2],
                strand: vec![b'+'; 4], mapping_quality: vec![30; 4],
                ..Default::default()
            };
            assert_eq!(pool.run(rows, &mut writer)?, 2);
            assert!(pool.workers.iter().all(|w| !w.is_finished()));
            assert_eq!(pool.workers.iter().map(|w| w.thread().id()).collect::<Vec<_>>(), ids);
            expected.extend([format!("{}:0:1", batch * 2), format!("{}:0:1", batch * 2 + 1)]);
        }
        drop(pool);
        let counts = writer.finish()?;
        assert_eq!((counts.q0_records, counts.q1_records), (40, 40));
        let mut reader = Reader::open(&output, 0)?;
        let mut observed = Vec::new();
        while let Some(Batch::Pairs(rows)) = reader.next_batch()? {
            observed.extend(rows.into_iter().map(|r| r.read_id));
        }
        assert_eq!(observed, expected);
        Ok(())
    }

    #[test]
    fn parallel_errors_cancel_blocked_workers() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output")
            .join(format!("convert-errors-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
        }
        let _cleanup = Cleanup(root.clone());
        let options = ConvertOptions { threads: 2, batch_rows: 1,
            chunk_size: 1, ..Default::default() };
        // The second worker produces many batches and blocks on its queue
        // while the first worker/consumer fails. Both must exit before cleanup.
        for worker_error in [true, false] {
            let mut rows = ConcatColumns {
                read_offsets: vec![0, 20, 40],
                ..Default::default()
            };
            for id in 0..2 {
                for j in 0..20 {
                    rows.read_idx.push(id);
                    rows.read_start.push(j);
                    rows.chrom.push(0);
                    rows.start.push(j as u64);
                    rows.end.push(j as u64 + 1);
                    rows.strand.push(b'+');
                    rows.mapping_quality.push(30);
                }
            }
            if worker_error { rows.end[0] = 0; } else { rows.chrom[0] = 99; }
            let output = root.join(if worker_error { "worker" } else { "writer" });
            let mut writer = Writer::create(&output, Kind::Pairs,
                vec![Contig { name: "a".into(), length: 100 }], 1)?;
            writer.parallel_encoding(2)?;
            let mut pool = ConversionPool::new(options)?;
            let error = pool.run(rows, &mut writer).unwrap_err();
            assert!(pool.failed);
            drop(pool);
            if worker_error { assert!(error.to_string().contains("end > start")); }
            drop(writer);
            assert!(!output.exists());
            assert!(!output.with_extension("partial").exists());
        }
        Ok(())
    }

    #[test]
    fn encoding_error_prevents_publication_and_cleans_staging() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output")
            .join(format!("convert-encode-error-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
        }
        let _cleanup = Cleanup(root.clone());
        let output = root.join("pairs");
        let mut writer = Writer::create(&output, Kind::Pairs,
            vec![Contig { name: "a".into(), length: 100 }], 1)?;
        writer.parallel_encoding(2)?;
        // An actual filesystem error inside the encoder, after submission.
        fs::create_dir(writer.staging.join("q0/0.parquet"))?;
        let pairs = PairColumns {
            read_id_offsets: vec![0, 1], read_id_bytes: b"r".to_vec(),
            chrom1: vec![0], chrom2: vec![0], pos1: vec![1], pos2: vec![2],
            strand1: vec![b'+'], strand2: vec![b'-'], mapq: vec![30],
        };
        writer.write_pairs_columns(pairs.as_view())?;
        assert!(writer.finish().is_err());
        drop(writer);
        assert!(!output.exists());
        assert!(!output.with_extension("partial").exists());
        Ok(())
    }
}
