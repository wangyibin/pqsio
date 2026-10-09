# Rust API

Add pqsio from crates.io to your crate's `Cargo.toml`:

```toml
[dependencies]
pqsio = "0.2.5"
```

## Read and write pairs

This complete program creates a small dataset and reads it back. The output
path must be new.

```rust
use pqsio::{Contig, Kind, Pair, Reader, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Writer::create(
        "sample.pairs.pqs", Kind::Pairs,
        vec![Contig { name: "chr1".into(), length: 1000 }], 100_000,
    )?;
    writer.write_pairs(&[Pair {
        read_id: "read1".into(), chrom1: 0, pos1: 10, chrom2: 0, pos2: 200,
        strand1: b'+', strand2: b'-', mapq: 60,
    }])?;
    writer.finish()?;

    let mut reader = Reader::open("sample.pairs.pqs", 0)?;
    while let Some(batch) = reader.next_batch()? {
        println!("{batch:?}");
    }
    Ok(())
}
```

For concat, use `Kind::Concat` and `writer.write_read(&[Alignment, ...])`.
Submit each complete read once, with strictly increasing IDs. Pairs positions
are 1-based; concat intervals are 0-based, half-open.

## Other operations

| Task | Rust entry point |
| --- | --- |
| Complete-read selection / merge | `subset`, `SubsetOptions` / `merge`, `MergeOptions` |
| Concat → pairs | `convert(..., "concat2pairs", ConvertOptions)` |
| BAM/PAF import | `import_alignments(..., mode, ImportOptions)` |
| Pairs → Cooler | `pairs2cool(..., CoolOptions)` |
| Streaming / region query | `StreamingReader` / `QueryReader` |

See [Rust API reference](rust-reference.md) for compression and conversion examples,
and the [reference index](reference.md) for full operation contracts.
