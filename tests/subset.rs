use pqsio::{subset, Batch, Contig, Kind, Pair, ReadIds, Reader, SubsetOptions, Writer};

#[test]
fn public_subset_preserves_fields_and_counts() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
    std::fs::create_dir_all(&root)?;
    let root = root.join(format!("subset-rust-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let input = root.join("input");
    let mut writer = Writer::create(
        &input,
        Kind::Pairs,
        vec![Contig {
            name: "a".into(),
            length: 100,
        }],
        2,
    )?;
    let row = Pair {
        read_id: "repeat".into(),
        chrom1: 0,
        pos1: 1,
        chrom2: 0,
        pos2: 100,
        strand1: b'+',
        strand2: b'-',
        mapq: 255,
    };
    writer.write_pairs(&[row.clone(), row.clone()])?;
    writer.finish()?;
    let result = subset(
        &input,
        root.join("output"),
        SubsetOptions {
            read_ids: Some(ReadIds::Pairs(vec!["repeat".into(), "repeat".into()])),
            min_mapq: Some(255),
            batch_rows: 1,
            chunk_size: 2,
            ..SubsetOptions::default()
        },
    )?;
    assert_eq!(result.counts.q0_records, 2);
    assert_eq!(result.counts.q1_records, 2);
    assert_eq!(result.scanned_records, 2);
    let mut reader = Reader::open(&result.output, 0)?;
    assert_eq!(
        reader.next_batch()?,
        Some(Batch::Pairs(vec![row.clone(), row]))
    );
    assert!(reader.next_batch()?.is_none());
    assert!(result.provenance.unwrap().is_file());
    Ok(())
}
