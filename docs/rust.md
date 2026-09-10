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
