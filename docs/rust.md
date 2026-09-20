# Rust API

Add `pqsio = { path = "../pqsio" }` to the consuming crate's dependencies.

```rust
use pqsio::{Contig, Kind, Pair, Writer, Reader};
# fn example() -> anyhow::Result<()> {
let mut writer = Writer::create("sample.pairs.pqs", Kind::Pairs,
    vec![Contig { name: "chr1".into(), length: 1000 }], 100_000)?;
writer.write_pairs(&[Pair {
    read_id: "read1".into(), chrom1: 0, pos1: 10, chrom2: 0, pos2: 200,
    strand1: b'+', strand2: b'-', mapq: 60,
}])?;
writer.finish()?;
let mut reader = Reader::open("sample.pairs.pqs", 0)?;
while let Some(batch) = reader.next_batch()? {
    // Consume Batch::Pairs or Batch::Concat.
}
# Ok(()) }
```

For concat, create with `Kind::Concat` and call `write_read(&[Alignment, ...])`
with **one complete read per call**, in strictly increasing global read-ID
order. For bulk submission, use `write_reads(&rows, &offsets)`; offsets are
`[0, ..., rows.len()]` and delimit complete nonempty reads. The owned variants
`write_pairs_owned(Vec<Pair>)` and `write_reads_owned(Vec<Alignment>, &offsets)`
transfer strings without cloning them. The same read must never be submitted twice. This contract avoids an
unbounded set of previously seen read IDs and prevents accidental group splits.

## Compression

`Writer::create` keeps default Zstd compression. Use a validated
`Compression` with `create_with_compression` to choose the codec and level:

```rust
use pqsio::{Compression, Contig, Kind, Writer};
# fn example() -> anyhow::Result<()> {
let mut writer = Writer::create_with_compression(
    "compressed.pairs.pqs", Kind::Pairs,
    vec![Contig { name: "chr1".into(), length: 1000 }], 100_000,
    Compression::new("zstd", Some(6))?,
)?;
writer.finish()?;
# Ok(()) }
```

Use `Compression::new("uncompressed", None)?` for uncompressed Parquet pages.
`ParallelWriter::create_with_compression` takes the same `Compression` as its
last argument, after `ParallelOptions`. See [codecs and levels](pqs-format.md#compression).

## Native conversions

Rust exposes three entry points; unlike Python/CLI, `convert` itself supports
only `concat2pairs`. Each returns `anyhow::Result` and a report with `.to_json()`.

```rust
use pqsio::{convert, import_alignments, pairs2cool,
            ConvertOptions, ImportOptions, CoolOptions};

fn conversions() -> anyhow::Result<()> {
    let pairs = convert("reads.concat.pqs", "midpoint.pairs.pqs", "concat2pairs",
        ConvertOptions { min_mapq: 1, threads: 4, ..Default::default() })?;
    println!("{}", pairs.to_json());

    let imported = import_alignments("hic.bam", "hic.pairs.pqs", "bam2pairs",
        ImportOptions { min_mapq: 1, threads: 4, ..Default::default() })?;
    println!("{}", imported.to_json());

    let cool = pairs2cool("hic.pairs.pqs", "hic.10k.cool",
        CoolOptions { bin_size: 10_000, min_mapq: 1, threads: 4,
                      chunk_size: 4_000_000, ..Default::default() })?;
    println!("{}", cool.to_json());
    Ok(())
}
```

| Options | Fields and defaults |
| --- | --- |
| All three | `threads: 1`, `chunk_size: 1_000_000`, `batch_rows: 65_536`, `min_mapq: 0` |
| `ConvertOptions` | `min_order: 2`, `max_order: usize::MAX` (exclusive) |
| `ImportOptions` | `min_order: None` (2 for pairs, 1 for concat), `max_order: None`, `contigsizes: None`, `include_secondary: false`, `tmpdir: None`, `five_prime: false` |
| `CoolOptions` | `bin_size: 10_000`, `contigsizes: None`, `tmpdir: None` |

`ImportOptions` supports `bam2pairs`, `bam2concat`, `paf2pairs`, `paf2concat`.
Optional paths are `Option<PathBuf>`. For direct pairs imports, `five_prime=true`
selects aligned 5′ ends; default positions are 1-based leftmost. Cooler takes
integer bp (`u64`, positive and at most `i64::MAX`), not `10k` strings.
Thread counts are per stage; all Cooler HDF5 calls remain on the caller even
when pixel compression uses workers. Full semantics are in [conversion](convert.md),
[imports](import.md), [Cooler](cool.md), and the [API overview](api.md).

## Summaries, export and progress

`pqsio::presentation::info(path, stats)` and `presentation::export(input,
output: Option<&Path>, ExportOptions)` return `metadata::Value` reports.
`ExportOptions::default()` exports all records, choosing pairs text for pairs
and headerless 11-column concat text for concat. A `None` output writes to stdout. Filters live in
`ExportOptions.query`; selected output columns live in `columns`.
See [browse and export](browse.md) for defaults and the optional thread-local
`pqsio::progress::set` callback lifetime contract.

`pqsio::statistics::stats(path, min_mapq)` returns a q0 quality summary as
`metadata::Value`; see [statistics](stats.md). `pqsio::query::index_status(path,
IndexQuality::Q0)` returns availability/identity status for that partition.
`presentation::export` respects region-query index and concat selection options
in `ExportOptions.query`, and includes `query_stats` in its result for queries.
