//! Small deterministic acceptance benchmark. 0 workers selects synchronous I/O.
use pqsio::*;
use std::{path::PathBuf, time::Instant};
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 5,
        "usage: parallel_bench OUTPUT pairs|concat WORKERS ROWS"
    );
    let path = PathBuf::from(&args[1]);
    let kind = match args[2].as_str() {
        "pairs" => Kind::Pairs,
        "concat" => Kind::Concat,
        _ => anyhow::bail!("invalid kind"),
    };
    let workers: usize = args[3].parse()?;
    let n: usize = args[4].parse()?;
    let chunk = 25_000;
    let contigs: Vec<Contig> = (0..64)
        .map(|i| Contig {
            name: format!("chr{i}"),
            length: 1_000_000_000,
        })
        .collect();
    let mut batches = Vec::new();
    for start in (0..n).step_by(chunk) {
        let end = n.min(start + chunk);
        batches.push(match kind {
            Kind::Pairs => Batch::Pairs((start..end).map(pair).collect()),
            Kind::Concat => Batch::Concat((start..end).map(alignment).collect()),
        });
    }
    let began = Instant::now();
    let counts = if workers == 0 {
        let mut w = Writer::create(&path, kind, contigs, chunk)?;
        for batch in batches {
            match batch {
                Batch::Pairs(rows) => w.write_pairs_owned(rows)?,
                Batch::Concat(rows) => {
                    let offsets = offsets(rows.len());
                    w.write_reads_owned(rows, &offsets)?;
                }
            }
        }
        w.finish()?
    } else {
        let mut w = ParallelWriter::create(
            &path,
            kind,
            contigs,
            chunk,
            ParallelOptions {
                workers,
                queue_capacity: 4,
                max_batch_bytes: 16 * 1024 * 1024,
            },
        )?;
        // Four independent producers; all data generation is outside timing.
        let mut lanes: Vec<Vec<(u64, Batch)>> = (0..4).map(|_| vec![]).collect();
        for (i, batch) in batches.into_iter().enumerate() {
            lanes[i % 4].push((i as u64, batch));
        }
        let result: anyhow::Result<()> = std::thread::scope(|scope| {
            let mut handles = vec![];
            for lane in lanes {
                let p = w.producer();
                handles.push(scope.spawn(move || -> anyhow::Result<()> {
                    for (i, batch) in lane {
                        match batch {
                            Batch::Pairs(rows) => p.write_pairs(i, rows)?,
                            Batch::Concat(rows) => {
                                let offsets = offsets(rows.len());
                                p.write_reads(i, rows, offsets)?;
                            }
                        }
                    }
                    Ok(())
                }));
            }
            for h in handles {
                h.join().expect("producer panic")?;
            }
            Ok(())
        });
        result?;
        w.finish()?
    };
    let elapsed = began.elapsed().as_secs_f64();
    let rss = std::fs::read_to_string("/proc/self/status")?
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .unwrap_or("")
        .to_string();
    // Exact full-field comparisons, including filtered views, outside timing.
    for quality in [0, 1, 20] {
        let mut reader = Reader::open(&path, quality)?;
        let mut expected = (0..n).filter(|i| ((*i % 61) as u8) >= quality);
        while let Some(batch) = reader.next_batch()? {
            match batch {
                Batch::Pairs(rows) => {
                    for row in rows {
                        assert_eq!(row, pair(expected.next().expect("extra row")));
                    }
                }
                Batch::Concat(rows) => {
                    for row in rows {
                        assert_eq!(row, alignment(expected.next().expect("extra row")));
                    }
                }
            }
        }
        assert!(expected.next().is_none(), "missing rows");
    }
    assert_eq!(counts.q0_records, n as u64);
    assert_eq!(
        counts.q1_records,
        (0..n).filter(|i| i % 61 > 0).count() as u64
    );
    if kind == Kind::Concat {
        assert_eq!(counts.q0_concats, n.div_ceil(5) as u64);
        assert_eq!(
            counts.q1_concats,
            (0..n)
                .step_by(5)
                .filter(|start| (*start..n.min(*start + 5)).any(|i| i % 61 > 0))
                .count() as u64
        );
    }
    println!(
        "seconds={elapsed:.6} rss_kib={} rows={n} verified=true",
        rss.split_whitespace().nth(1).unwrap_or("0")
    );
    Ok(())
}
fn offsets(n: usize) -> Vec<usize> {
    (0..n).step_by(5).chain(std::iter::once(n)).collect()
}
fn pair(i: usize) -> Pair {
    Pair {
        read_id: format!("read-{i:012}"),
        chrom1: (i % 64) as u32,
        pos1: (i * 7919 % 999_999_000 + 1) as u64,
        chrom2: ((i * 17 + 3) % 64) as u32,
        pos2: (i * 3571 % 999_999_000 + 1) as u64,
        strand1: if i.is_multiple_of(2) { b'+' } else { b'-' },
        strand2: b'+',
        mapq: (i % 61) as u8,
    }
}
fn alignment(i: usize) -> Alignment {
    Alignment {
        read_idx: (i / 5) as u64,
        read_length: 1000,
        read_start: ((i % 5) * 100) as u32,
        read_end: ((i % 5) * 100 + 80) as u32,
        strand: if i.is_multiple_of(2) { b'+' } else { b'-' },
        chrom: (i % 64) as u32,
        start: (i * 7919 % 999_999_000) as u64,
        end: (i * 7919 % 999_999_000 + 80) as u64,
        mapping_quality: (i % 61) as u8,
        identity: 0.95,
        filter_reason: if i.is_multiple_of(7) { "low" } else { "pass" }.into(),
    }
}
