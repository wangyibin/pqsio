use pqsio::{inspect, validate, Contig, Kind, Pair, ValidationLevel, Writer};

#[test]
fn rust_inspection_and_validation_are_read_only() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
    std::fs::create_dir_all(&root)?;
    let path = root.join(format!("inspection-rust-{}", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    let mut w = Writer::create(
        &path,
        Kind::Pairs,
        vec![Contig {
            name: "a".into(),
            length: 100,
        }],
        1,
    )?;
    w.write_pairs(&[Pair {
        read_id: "r".into(),
        chrom1: 0,
        pos1: 1,
        chrom2: 0,
        pos2: 100,
        strand1: b'+',
        strand2: b'-',
        mapq: 60,
    }])?;
    w.finish()?;
    let info = inspect(&path)?;
    assert_eq!(info.shards.len(), 2);
    assert_eq!(info.declared_counts["q0_records"].u64(), Some(1));
    assert!(info.to_json().contains("checks_not_performed"));
    for level in [ValidationLevel::Quick, ValidationLevel::Full] {
        let report = validate(&path, level, 10)?;
        assert_eq!(report.status, "valid", "{}", report.to_json());
    }
    assert!(validate(&path, ValidationLevel::Quick, 0).is_err());
    Ok(())
}
