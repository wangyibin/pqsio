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
