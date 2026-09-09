//! Deterministic synthetic I/O benchmark. stdout is a single JSON record.
//! Generator/checksum time is excluded from io_seconds; process RSS includes it.
use pqsio::{Alignment, Batch, Contig, Kind, Pair, Reader, Writer};
use std::{env, time::Instant};
fn random(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn contigs() -> Vec<Contig> {
    (0..256)
        .map(|i| Contig {
            name: format!("contig_{i:04}"),
            length: 100_000_000,
        })
        .collect()
}
fn quality(x: u64) -> u8 {
    [0, 1, 10, 30, 60][(x % 5) as usize]
}
fn pair(i: u64) -> Pair {
    let a = random(i ^ 0xc0ffee);
    let b = random(a);
    Pair {
        read_id: format!("read_{i:016x}"),
        chrom1: (a % 256) as u32,
        pos1: a % 99_999_000 + 1,
        chrom2: (b % 256) as u32,
        pos2: b % 99_999_000 + 1,
        strand1: if a & 256 == 0 { b'+' } else { b'-' },
        strand2: if b & 256 == 0 { b'+' } else { b'-' },
        mapq: quality(a >> 16),
    }
}
fn alignment(read: u64, segment: u32, segments: u32) -> Alignment {
    let a = random(read.wrapping_mul(31).wrapping_add(segment as u64) ^ 0xc0ffee);
    let start = a % 99_999_000;
    Alignment {
        read_idx: read,
        read_length: segments * 1000,
        read_start: segment * 1000,
        read_end: segment * 1000 + 950,
        strand: if a & 256 == 0 { b'+' } else { b'-' },
        chrom: (a % 256) as u32,
        start,
        end: start + 950,
        mapping_quality: quality(a >> 16),
        identity: 0.85 + ((a >> 24) % 1500) as f32 / 10000.0,
        filter_reason: "pass".into(),
    }
}
fn hash_bytes(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h = (*h ^ *b as u64).wrapping_mul(1099511628211);
    }
}
fn hash_num(h: &mut u64, n: u64) {
    hash_bytes(h, &n.to_le_bytes());
}
fn hash_string(h: &mut u64, s: &str) {
    hash_num(h, s.len() as u64);
    hash_bytes(h, s.as_bytes());
}
fn digest(h: &mut u64, batch: &Batch) -> usize {
    match batch {
        Batch::Pairs(rows) => {
            for r in rows {
                hash_string(h, &r.read_id);
                for n in [
                    r.chrom1 as u64,
                    r.pos1,
                    r.chrom2 as u64,
                    r.pos2,
                    r.strand1 as u64,
                    r.strand2 as u64,
                    r.mapq as u64,
                ] {
                    hash_num(h, n);
                }
            }
            rows.len()
        }
        Batch::Concat(rows) => {
            for r in rows {
                for n in [
                    r.read_idx,
                    r.read_length as u64,
                    r.read_start as u64,
                    r.read_end as u64,
                    r.strand as u64,
                    r.chrom as u64,
                    r.start,
                    r.end,
                    r.mapping_quality as u64,
                    r.identity.to_bits() as u64,
                ] {
                    hash_num(h, n);
                }
                hash_string(h, &r.filter_reason);
            }
            rows.len()
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 6 {
        return Err(
            "usage: version_bench write|read pairs|concat PATH ROWS_OR_MAPQ CHUNKSIZE".into(),
        );
    }
    let kind = match args[2].as_str() {
        "pairs" => Kind::Pairs,
        "concat" => Kind::Concat,
        _ => return Err("unknown kind".into()),
    };
    let path = &args[3];
    let size: usize = args[4].parse()?;
    let chunk: usize = args[5].parse()?;
    let mut elapsed = 0.0;
    let mut hash = 14695981039346656037u64;
    let mut count = 0;
    if args[1] == "write" {
        let now = Instant::now();
        let mut writer = Writer::create(path, kind, contigs(), chunk)?;
        elapsed += now.elapsed().as_secs_f64();
        match kind {
            Kind::Pairs => {
                while count < size {
                    let rows: Vec<Pair> = (count..(count + 8192).min(size))
                        .map(|i| pair(i as u64))
                        .collect();
                    let now = Instant::now();
                    writer.write_pairs(&rows)?;
                    elapsed += now.elapsed().as_secs_f64();
                    count += digest(&mut hash, &Batch::Pairs(rows));
                }
            }
            Kind::Concat => {
                let mut read_id = 1;
                while count < size {
                    let segments = (3 + read_id as usize % 12).min(size - count) as u32;
                    let rows = (0..segments)
                        .map(|s| alignment(read_id, s, segments))
                        .collect::<Vec<_>>();
                    let now = Instant::now();
                    writer.write_read(&rows)?;
                    elapsed += now.elapsed().as_secs_f64();
                    count += digest(&mut hash, &Batch::Concat(rows));
                    read_id += 1;
                }
            }
        }
        let now = Instant::now();
        let counts = writer.finish()?;
        elapsed += now.elapsed().as_secs_f64();
        assert_eq!(counts.q0_records, count as u64);
    } else if args[1] == "read" {
        let now = Instant::now();
        let mut reader = Reader::open(path, size.try_into()?)?;
        elapsed += now.elapsed().as_secs_f64();
        assert_eq!(reader.kind, kind);
        loop {
            let now = Instant::now();
            let batch = reader.next_batch()?;
            elapsed += now.elapsed().as_secs_f64();
            match batch {
                Some(b) => count += digest(&mut hash, &b),
                None => break,
            }
        }
    } else {
        return Err("unknown mode".into());
    }
    println!("{{\"io_seconds\":{elapsed:.9},\"records\":{count},\"digest\":\"{hash:016x}\"}}");
    Ok(())
}
