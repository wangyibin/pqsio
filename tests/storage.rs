use pqsio::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static ID: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/output")
            .join(format!(
                "rust-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn contigs() -> Vec<Contig> {
    vec![
        Contig {
            name: "chr1".into(),
            length: 1000,
        },
        Contig {
            name: "chr2".into(),
            length: u32::MAX as u64 + 100,
        },
    ]
}
fn pair(q: u8) -> Pair {
    Pair {
        read_id: format!("read{q}"),
        chrom1: 0,
        pos1: 10,
        chrom2: 1,
        pos2: u32::MAX as u64 + 1,
        strand1: b'+',
        strand2: b'-',
        mapq: q,
    }
}
fn alignment(id: u64, q: u8) -> Alignment {
    Alignment {
        read_idx: id,
        read_length: 100,
        read_start: 0,
        read_end: 50,
        strand: b'+',
        chrom: 0,
        start: 0,
        end: 50,
        mapping_quality: q,
        identity: 0.95,
        filter_reason: "pass".into(),
    }
}
#[test]
fn pairs_roundtrip_long_positions_and_quality() {
    let s = Scratch::new();
    let p = s.0.join("pairs.pqs");
    let rows = vec![pair(0), pair(1), pair(30)];
    let mut w = Writer::create(&p, Kind::Pairs, contigs(), 2).unwrap();
    w.write_pairs(&rows).unwrap();
    assert!(!p.exists());
    let counts = w.finish().unwrap();
    assert_eq!(counts.q0_records, 3);
    assert_eq!(counts.q1_records, 2);
    assert!(w.finish().is_err());
    let mut r = Reader::open(&p, 0).unwrap();
    let mut all = vec![];
    while let Some(Batch::Pairs(rs)) = r.next_batch().unwrap() {
        all.extend(rs);
    }
    assert_eq!(all, rows);
    let mut r = Reader::open(&p, 20).unwrap();
    let mut all = vec![];
    while let Some(Batch::Pairs(rs)) = r.next_batch().unwrap() {
        all.extend(rs);
    }
    assert_eq!(all, vec![pair(30)]);
    assert!(Writer::create(&p, Kind::Pairs, contigs(), 2).is_err());
}
#[test]
fn concat_complete_reads_oversize_and_counts() {
    let s = Scratch::new();
    let p = s.0.join("concat.pqs");
    let mut w = Writer::create(&p, Kind::Concat, contigs(), 2).unwrap();
    let one = vec![alignment(10, 0), alignment(10, 20), alignment(10, 1)];
    w.write_read(&one).unwrap();
    w.write_read(&[alignment(20, 0)]).unwrap();
    assert!(w.write_read(&[alignment(10, 1)]).is_err());
    let c = w.finish().unwrap();
    assert_eq!(
        (c.q0_records, c.q1_records, c.q0_concats, c.q1_concats),
        (4, 2, 2, 1)
    );
    let mut r = Reader::open(&p, 0).unwrap();
    assert_eq!(r.next_batch().unwrap(), Some(Batch::Concat(one)));
    assert_eq!(
        r.next_batch().unwrap(),
        Some(Batch::Concat(vec![alignment(20, 0)]))
    );
    assert!(r.next_batch().unwrap().is_none());
}
#[test]
fn validation_is_atomic_and_drop_aborts() {
    let s = Scratch::new();
    let p = s.0.join("aborted.pqs");
    {
        let mut w = Writer::create(&p, Kind::Pairs, contigs(), 1).unwrap();
        let mut invalid = pair(1);
        invalid.pos1 = 0;
        assert!(w.write_pairs(&[pair(0), invalid]).is_err());
        assert_eq!(w.finish().unwrap().q0_records, 0);
    }
    assert!(p.exists());
    let p = s.0.join("unfinished.pqs");
    {
        let mut w = Writer::create(&p, Kind::Pairs, contigs(), 1).unwrap();
        w.write_pairs(&[pair(1)]).unwrap();
    }
    assert!(!p.exists());
    assert!(!s.0.join("unfinished.pqs.partial").exists());
}
#[test]
fn reject_mixed_or_invalid_concat_read() {
    let s = Scratch::new();
    let mut w = Writer::create(s.0.join("c.pqs"), Kind::Concat, contigs(), 2).unwrap();
    assert!(w.write_read(&[alignment(1, 1), alignment(2, 1)]).is_err());
    let mut a = alignment(1, 1);
    a.end = 1001;
    assert!(w.write_read(&[a]).is_err());
    w.write_read(&[alignment(1, 1)]).unwrap();
    assert_eq!(w.finish().unwrap().q0_records, 1);
}
#[test]
fn output_race_does_not_overwrite() {
    let s = Scratch::new();
    let p = s.0.join("race.pqs");
    let mut w = Writer::create(&p, Kind::Pairs, contigs(), 2).unwrap();
    fs::create_dir(&p).unwrap();
    fs::write(p.join("keep"), "untouched").unwrap();
    assert!(w.finish().is_err());
    assert_eq!(fs::read_to_string(p.join("keep")).unwrap(), "untouched");
}

#[test]
fn bulk_reads_preserve_groups_and_counts_across_calls() {
    let s = Scratch::new();
    let path = s.0.join("bulk.pqs");
    let mut w = Writer::create(&path, Kind::Concat, contigs(), 3).unwrap();
    let rows = vec![
        alignment(1, 0),
        alignment(1, 10),
        alignment(2, 0),
        alignment(3, 0),
        alignment(3, 0),
        alignment(3, 1),
        alignment(3, 30),
    ];
    w.write_reads(&rows, &[0, 2, 3, 7]).unwrap();
    w.write_reads_owned(vec![alignment(4, 0), alignment(4, 1)], &[0, 2])
        .unwrap();
    w.write_read(&[alignment(5, 0)]).unwrap();
    let c = w.finish().unwrap();
    assert_eq!(
        (c.q0_records, c.q1_records, c.q0_concats, c.q1_concats),
        (10, 4, 5, 3)
    );
    let mut r = Reader::open(&path, 0).unwrap();
    let mut lengths = vec![];
    while let Some(Batch::Concat(batch)) = r.next_batch().unwrap() {
        lengths.push(batch.len());
    }
    assert_eq!(lengths, vec![3, 4, 3]);
}
#[test]
fn invalid_bulk_batch_is_not_partially_accepted() {
    let s = Scratch::new();
    let mut w = Writer::create(s.0.join("bulk.pqs"), Kind::Concat, contigs(), 1).unwrap();
    let rows = vec![alignment(1, 1), alignment(2, 0)];
    for offsets in [
        vec![],
        vec![1, 2],
        vec![0, 1],
        vec![0, 0, 2],
        vec![0, 3, 2],
        vec![0, usize::MAX, 2],
    ] {
        assert!(w.write_reads(&rows, &offsets).is_err());
    }
    let mut bad = rows.clone();
    bad[1].read_end = 101;
    assert!(w.write_reads_owned(bad, &[0, 1, 2]).is_err());
    assert!(w.write_reads(&rows, &[0, 2]).is_err()); // mixed IDs within a read
    assert!(w
        .write_reads(&[alignment(2, 1), alignment(1, 1)], &[0, 1, 2])
        .is_err());
    w.write_reads_owned(rows, &[0, 1, 2]).unwrap();
    assert!(w.write_reads(&[alignment(2, 1)], &[0, 1]).is_err());
    let c = w.finish().unwrap();
    assert_eq!(
        (c.q0_records, c.q1_records, c.q0_concats, c.q1_concats),
        (2, 1, 2, 1)
    );
}
#[test]
fn owned_pairs_keep_order_and_validate_before_flushing() {
    let s = Scratch::new();
    let p = s.0.join("pairs.pqs");
    let mut w = Writer::create(&p, Kind::Pairs, contigs(), 2).unwrap();
    let mut bad = pair(2);
    bad.chrom2 = 500;
    assert!(w.write_pairs_owned(vec![pair(1), bad]).is_err());
    w.write_pairs(&[pair(0)]).unwrap();
    w.write_pairs_owned(vec![pair(1), pair(2), pair(3)])
        .unwrap();
    assert_eq!(w.finish().unwrap().q0_records, 4);
    let mut r = Reader::open(&p, 0).unwrap();
    let mut all = vec![];
    while let Some(Batch::Pairs(rows)) = r.next_batch().unwrap() {
        all.extend(rows);
    }
    assert_eq!(all, vec![pair(0), pair(1), pair(2), pair(3)]);
}
#[test]
fn empty_bulk_is_noop_and_io_error_poisoning_survives() {
    let s = Scratch::new();
    let p = s.0.join("empty.pqs");
    let mut w = Writer::create(&p, Kind::Concat, contigs(), 1).unwrap();
    w.write_reads(&[], &[0]).unwrap();
    w.write_reads_owned(vec![], &[0]).unwrap();
    assert_eq!(w.finish().unwrap().q0_records, 0);
    let p = s.0.join("failure.pqs");
    let mut w = Writer::create(&p, Kind::Concat, contigs(), 1).unwrap();
    fs::create_dir(s.0.join("failure.pqs.partial/q0/0.parquet")).unwrap();
    assert!(w.write_reads_owned(vec![alignment(1, 1)], &[0, 1]).is_err());
    assert!(w.write_read(&[alignment(2, 1)]).is_err());
    assert!(w.finish().is_err());
    drop(w);
    assert!(!p.exists());
    assert!(!s.0.join("failure.pqs.partial").exists());
}

fn parallel_options() -> ParallelOptions {
    ParallelOptions {
        workers: 2,
        queue_capacity: 1,
        max_batch_bytes: 4096,
    }
}
#[test]
fn parallel_pairs_out_of_order_reserved_slot_and_counts() {
    let tmp = Scratch::new();
    let path = tmp.0.join("parallel");
    let mut w =
        ParallelWriter::create(&path, Kind::Pairs, contigs(), 2, parallel_options()).unwrap();
    let p = w.producer();
    // Fill the only future-batch slot; sequence zero must still be admitted.
    p.write_pairs(1, vec![pair(1)]).unwrap();
    let p2 = p.clone();
    let t = std::thread::spawn(move || p2.write_pairs(2, vec![pair(2)]));
    p.write_pairs(0, vec![pair(0)]).unwrap();
    t.join().unwrap().unwrap();
    let c = w.finish().unwrap();
    assert_eq!((c.q0_records, c.q1_records), (3, 2));
    let mut r = Reader::open(&path, 0).unwrap();
    let mut rows = Vec::new();
    while let Some(Batch::Pairs(b)) = r.next_batch().unwrap() {
        rows.extend(b);
    }
    assert_eq!(rows, vec![pair(0), pair(1), pair(2)]);
    assert!(p.write_pairs(3, vec![]).is_err());
    assert!(w.finish().is_err());
}
#[test]
fn parallel_concat_complete_reads_and_cross_batch_validation() {
    let tmp = Scratch::new();
    let path = tmp.0.join("concat");
    let mut w =
        ParallelWriter::create(&path, Kind::Concat, contigs(), 2, parallel_options()).unwrap();
    let p = w.producer();
    let rows = vec![
        alignment(1, 0),
        alignment(1, 1),
        alignment(1, 20),
        alignment(2, 0),
    ];
    p.write_reads(0, rows.clone(), vec![0, 3, 4]).unwrap();
    p.write_reads(1, vec![alignment(3, 10)], vec![0, 1])
        .unwrap();
    let c = w.finish().unwrap();
    assert_eq!(
        (c.q0_records, c.q1_records, c.q0_concats, c.q1_concats),
        (5, 3, 3, 2)
    );
    let mut r = Reader::open(&path, 0).unwrap();
    assert_eq!(
        r.next_batch().unwrap(),
        Some(Batch::Concat(rows[..3].to_vec()))
    );
    assert_eq!(
        r.next_batch().unwrap(),
        Some(Batch::Concat(rows[3..].to_vec()))
    );
    assert_eq!(
        r.next_batch().unwrap(),
        Some(Batch::Concat(vec![alignment(3, 10)]))
    );
    let bad = tmp.0.join("bad");
    let mut w =
        ParallelWriter::create(&bad, Kind::Concat, contigs(), 2, parallel_options()).unwrap();
    let p = w.producer();
    p.write_reads(0, vec![alignment(4, 0)], vec![0, 1]).unwrap();
    let _ = p.write_reads(1, vec![alignment(3, 0)], vec![0, 1]);
    assert!(w
        .finish()
        .unwrap_err()
        .to_string()
        .contains("strictly increasing"));
    assert!(!bad.exists());
    assert!(!tmp.0.join("bad.partial").exists());
}
#[test]
fn parallel_gap_duplicate_size_empty_and_abort() {
    let tmp = Scratch::new();
    let path = tmp.0.join("gap");
    let mut w =
        ParallelWriter::create(&path, Kind::Pairs, contigs(), 2, parallel_options()).unwrap();
    let p = w.producer();
    let mut oversized = pair(0);
    oversized.read_id = "x".repeat(4096);
    assert!(p.write_pairs(0, vec![oversized]).is_err());
    p.write_pairs(1, vec![]).unwrap();
    assert!(p.write_pairs(1, vec![]).is_err());
    assert!(w
        .finish()
        .unwrap_err()
        .to_string()
        .contains("missing batch sequence 0"));
    assert!(!tmp.0.join("gap.partial").exists());
    let mut w = ParallelWriter::create(
        tmp.0.join("empty"),
        Kind::Pairs,
        contigs(),
        2,
        parallel_options(),
    )
    .unwrap();
    assert_eq!(w.finish().unwrap().q0_records, 0);
    let w = ParallelWriter::create(
        tmp.0.join("abort"),
        Kind::Pairs,
        contigs(),
        2,
        parallel_options(),
    )
    .unwrap();
    let p = w.producer();
    p.write_pairs(1, vec![pair(1)]).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        tx.send(p.write_pairs(2, vec![pair(2)]).is_err()).unwrap();
    });
    drop(w);
    assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
    t.join().unwrap();
    assert!(!tmp.0.join("abort.partial").exists());
}
#[test]
fn parallel_worker_failure_wakes_idle_coordinator_and_producers() {
    let tmp = Scratch::new();
    let path = tmp.0.join("io");
    let mut w =
        ParallelWriter::create(&path, Kind::Pairs, contigs(), 2, parallel_options()).unwrap();
    let p = w.producer();
    // A directory at a file destination deterministically fails worker I/O.
    fs::create_dir(tmp.0.join("io.partial/q0/0.parquet")).unwrap();
    p.write_pairs(0, vec![pair(0)]).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        // Future batches eventually block, then must wake on the worker error.
        let result = p
            .write_pairs(2, vec![])
            .and_then(|_| p.write_pairs(3, vec![]));
        tx.send(result.is_err()).unwrap();
    });
    assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
    t.join().unwrap();
    assert!(w.finish().is_err());
    assert!(!path.exists());
    assert!(!tmp.0.join("io.partial").exists());
}

#[test]
fn columnar_owned_results_and_mixed_submission() -> anyhow::Result<()> {
    let tmp = Scratch::new();
    let path = tmp.0.join("columns");
    let mut w = Writer::create(&path, Kind::Pairs, contigs(), 2)?;
    let b = PairColumns {
        read_id_offsets: vec![0, 0, 3],
        read_id_bytes: "读".as_bytes().to_vec(),
        chrom1: vec![0, 0],
        pos1: vec![1, 2],
        chrom2: vec![1, 1],
        pos2: vec![u32::MAX as u64 + 1; 2],
        strand1: vec![b'+'; 2],
        strand2: vec![b'-'; 2],
        mapq: vec![0, 60],
    };
    w.write_pairs_columns(b.as_view())?;
    w.write_pairs(&[pair(1)])?;
    assert_eq!(w.finish()?.q0_records, 3);
    let mut r = Reader::open(&path, 0)?;
    let first = r.next_columns()?.unwrap();
    drop(r);
    assert_eq!(first, ColumnBatch::Pairs(b));
    let mut r = Reader::open(&path, 1)?;
    let Some(ColumnBatch::Pairs(b)) = r.next_columns()? else {
        panic!()
    };
    assert_eq!(b.mapq, [60]);
    assert_eq!(b.read_id_bytes, "读".as_bytes());
    Ok(())
}

#[test]
fn columnar_concat_validates_whole_batch_and_owned_lifetime() -> anyhow::Result<()> {
    let tmp = Scratch::new();
    let path = tmp.0.join("concat-columns");
    let mut w = Writer::create(&path, Kind::Concat, contigs(), 1)?;
    let mut b = ConcatColumns {
        read_offsets: vec![0, 2, 3],
        read_idx: vec![1, 1, 2],
        read_length: vec![100; 3],
        read_start: vec![0; 3],
        read_end: vec![50; 3],
        strand: vec![b'+'; 3],
        chrom: vec![0; 3],
        start: vec![0; 3],
        end: vec![50; 3],
        mapping_quality: vec![0, 60, 1],
        identity: vec![0.5; 3],
        filter_reason_offsets: vec![0, 0, 0, 0],
        filter_reason_bytes: vec![],
    };
    b.identity[2] = f32::NAN;
    assert!(w.write_concat_columns(b.as_view()).is_err());
    b.identity[2] = 0.5;
    w.write_concat_columns(b.as_view())?;
    assert!(w.write_read(&[alignment(2, 1)]).is_err());
    w.write_read(&[alignment(3, 1)])?;
    let counts = w.finish()?;
    assert_eq!(
        (
            counts.q0_records,
            counts.q1_records,
            counts.q0_concats,
            counts.q1_concats
        ),
        (4, 3, 3, 3)
    );
    let mut r = Reader::open(path, 0)?;
    let Some(ColumnBatch::Concat(first)) = r.next_columns()? else {
        panic!()
    };
    drop(r);
    assert_eq!(first.read_idx, [1, 1]);
    assert_eq!(first.read_offsets, [0, 2]);
    Ok(())
}
