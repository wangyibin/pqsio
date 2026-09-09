//! Opt-in internal benchmark; no production API or runtime instrumentation.
use super::*;
use std::{hint::black_box, time::Instant};

fn verify(actual: ColumnBatch, expected: &Batch) -> Result<()> {
    match (actual.into_rows()?, expected) {
        (Batch::Pairs(a), Batch::Pairs(b)) => assert_eq!(&a, b),
        (Batch::Concat(a), Batch::Concat(b)) => assert_eq!(&a, b),
        _ => panic!("wrong record kind"),
    }
    Ok(())
}

fn mode_name(mode: usize) -> &'static str {
    ["baseline", "typed", "chunk"][mode]
}
fn convert(reader: &Reader, frame: DataFrame, mode: usize) -> Result<ColumnBatch> {
    match mode {
        0 => reader.frame_columns_mode::<0>(frame),
        1 => reader.frame_columns_mode::<1>(frame),
        2 => reader.frame_columns(frame),
        _ => unreachable!(),
    }
}

fn emit(case: &str, phase: &str, chunks: usize, samples: &[f64]) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    println!("FRAME_BENCH {{\"case\":\"{case}\",\"phase\":\"{phase}\",\"chunks\":{chunks},\"rows\":80000,\"median_ms\":{},\"samples_ms\":{samples:?}}}", sorted[sorted.len() / 2]);
}

#[test]
#[ignore = "explicit small synthetic performance measurement"]
fn measure_frame_columns() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/output")
        .join(format!("frame-columns-{}", std::process::id()));
    fs::create_dir_all(&root)?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    for kind in [Kind::Pairs, Kind::Concat] {
        for wide in [false, true] {
            let case = format!(
                "{}_{}",
                if kind == Kind::Pairs {
                    "pairs"
                } else {
                    "concat"
                },
                if wide { "u64" } else { "u32" }
            );
            let base = if wide { 1u64 << 33 } else { 0 };
            let contigs = (0..64)
                .map(|i| Contig {
                    name: format!("ctg{i}"),
                    length: base + 1000000,
                })
                .collect();
            let expected = if kind == Kind::Pairs {
                Batch::Pairs(
                    (0..80000u64)
                        .map(|i| Pair {
                            read_id: format!("读段{i}"),
                            chrom1: (i % 64) as u32,
                            pos1: base + i + 1,
                            chrom2: ((i * 7) % 64) as u32,
                            pos2: base + i + 10,
                            strand1: b'+',
                            strand2: b'-',
                            mapq: (i % 61) as u8,
                        })
                        .collect(),
                )
            } else {
                Batch::Concat(
                    (0..80000u64)
                        .map(|i| Alignment {
                            read_idx: i / 8 + 1,
                            read_length: 800,
                            read_start: (i % 8 * 100) as u32,
                            read_end: (i % 8 * 100 + 90) as u32,
                            strand: if i % 2 == 0 { b'+' } else { b'-' },
                            chrom: (i % 64) as u32,
                            start: base + i,
                            end: base + i + 90,
                            mapping_quality: (i % 61) as u8,
                            identity: 0.5,
                            filter_reason: if i % 2 == 0 { "通过" } else { "" }.into(),
                        })
                        .collect(),
                )
            };
            let path = root.join(&case);
            let mut writer = Writer::create(&path, kind, contigs, 80000)?;
            match &expected {
                Batch::Pairs(rows) => writer.write_pairs(rows)?,
                Batch::Concat(rows) => {
                    for read in rows.chunks(8) {
                        writer.write_read(read)?;
                    }
                }
            }
            writer.finish()?;
            let mut reader = Reader::open(&path, 0)?;
            let mut frame = reader.next_frame()?.unwrap();
            frame.rechunk_mut();
            for chunks in [1, 8] {
                let mut input = frame.slice(0, 80000 / chunks);
                for i in 1..chunks {
                    input.vstack_mut(&frame.slice((i * 80000 / chunks) as i64, 80000 / chunks))?;
                }
                assert_eq!(input.get_columns()[0].n_chunks(), chunks);
                let mut times = [vec![], vec![], vec![]];
                for rep in 0..12 {
                    for offset in 0..3 {
                        let mode = (rep + offset) % 3;
                        let owned = input.clone();
                        let start = Instant::now();
                        let columns = black_box(convert(&reader, owned, mode)?);
                        let ms = start.elapsed().as_secs_f64() * 1000.;
                        verify(columns, &expected)?;
                        if rep >= 3 {
                            times[mode].push(ms);
                        }
                    }
                }
                for (mode, samples) in times.iter().enumerate() {
                    emit(
                        &case,
                        &format!("conversion_{}", mode_name(mode)),
                        chunks,
                        samples,
                    );
                }
            }
            let mut times = [vec![], vec![], vec![]];
            for rep in 0..12 {
                for offset in 0..3 {
                    let mode = (rep + offset) % 3;
                    let mut reader = Reader::open(&path, 0)?;
                    let start = Instant::now();
                    let columns = black_box(if mode == 2 {
                        reader.next_columns()?.unwrap()
                    } else {
                        let frame = reader.next_frame()?.unwrap();
                        convert(&reader, frame, mode)?
                    });
                    let ms = start.elapsed().as_secs_f64() * 1000.;
                    verify(columns, &expected)?;
                    if rep >= 3 {
                        times[mode].push(ms);
                    }
                }
            }
            for (mode, samples) in times.iter().enumerate() {
                emit(&case, &format!("read_{}", mode_name(mode)), 1, samples);
            }
        }
    }
    Ok(())
}
