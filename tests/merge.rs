use pqsio::{merge, Contig, Kind, MergeOptions, Pair, Writer};

#[test]
fn public_rust_merge() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
    std::fs::create_dir_all(&root)?;
    let root = root.join(format!("merge-rust-{}", std::process::id()));
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
            length: 10,
        }],
        1,
    )?;
    writer.write_pairs(&[Pair {
        read_id: "r".into(),
        chrom1: 0,
        pos1: 1,
        chrom2: 0,
        pos2: 2,
        strand1: b'+',
        strand2: b'-',
        mapq: 1,
    }])?;
    writer.finish()?;
    let output = root.join("merged");
    let result = merge(
        &[input],
        &output,
        MergeOptions {
            chunk_size: 1,
            batch_rows: 1,
            provenance: true,
        },
    )?;
    assert_eq!(result.counts.q0_records, 1);
    assert_eq!(result.counts.q1_records, 1);
    assert!(result.to_json().contains("_merge_sources.jsonl"));
    assert!(output.join("_merge_sources.jsonl").is_file());
    assert!(!root.join("merged.partial").exists());
    Ok(())
}
