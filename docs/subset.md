# Streaming subset

Python and Rust expose an additive `subset` API. It creates a new dataset of
exactly the input kind, using the native column pipeline and synchronous Writer.
No runtime dependencies, disk format change, CLI or format conversion are added.

```python
import pqsio
result = pqsio.subset(
    input="sample.pqs", output="subset.pqs",
    regions=[("chr1", 100_000, 200_000)], min_mapq=30,
    mode="complete_reads",  # concat only
    batch_rows=65_536, chunk_size=1_000_000, provenance=True,
)
print(result.to_dict())
```

```rust
use pqsio::{subset, SubsetOptions, Region, ConcatFilter};
# fn example() -> anyhow::Result<()> {
let result = subset("sample.pqs", "subset.pqs", SubsetOptions {
    regions: Some(vec![Region {
        contig: "chr1".into(), start: 100_000, end: 200_000,
    }]),
    min_mapq: Some(30), mode: Some(ConcatFilter::CompleteReads),
    ..SubsetOptions::default()
})?;
println!("{}", result.to_json());
# Ok(()) }
```

## Conditions and coordinates

Options are `min_mapq`, `chroms`, `regions`, `read_ids`, `pairs_mode`, `mode`,
`batch_rows`, `chunk_size`, `provenance`. Python lists/tuples are accepted for
list conditions; generators, filenames and expression strings are not accepted.
`None` / Rust `None` disables a condition. An explicit empty list matches nothing.
Values within a condition form a union; different conditions intersect. Repeated
IDs and overlapping regions never duplicate output. Original duplicate records
and traversal order remain intact.

MAPQ is an integer in 0..255 (Python booleans are rejected). Contig names must
exist, with no aliases. Regions are `(contig, start, end)` with UInt64 coordinates
and `0 <= start < end <= contig_length`. Concat uses interval overlap. Pairs
positions are interpreted as `position - 1` for selection only; output retains
its original 1-based coordinates. The region interval implementation is shared
with QueryReader.

For pairs, `pairs_mode="either"` (default) accepts at least one endpoint;
`"both"` requires both. Each endpoint independently satisfies the *entire*
spatial condition, including both chroms and regions if supplied. MAPQ and ID
conditions apply to the pair. Without spatial conditions the endpoint strategy
adds no restriction. Any explicit concat `mode` is rejected for pairs.

For concat, an explicit `pairs_mode` is rejected. The default
`mode="matching_alignments"` keeps only alignments satisfying every condition.
All retained alignments of a read are submitted as one complete Writer group;
this does not restore filtered alignments. `mode="complete_reads"` retains all
q0 alignments of a read when any *one* alignment satisfies every condition.
Conditions cannot be combined across different alignments. Low-MAPQ and
out-of-region alignments can consequently appear in the output. Reads spanning
row groups and global-ID shards remain complete. Oversized reads are never split.

## IDs, contigs and output

Pairs keep original string IDs; repeated records with the same ID are tested
separately. Concat accepts public Reader logical UInt64 IDs: global input IDs
are preserved, including zero and gaps. Legacy shard-local IDs are assigned by
Reader's deterministic q0 traversal *before selection*; equal local IDs from
different shards remain different reads. Output scope is global. Changing
batch size or filters does not change the logical mapping.

Rust uses `ReadIds::Pairs(Vec<String>)` or `ReadIds::Concat(Vec<u64>)`. Conditions
are capped at 100,000 entries (before deduplication); string payloads at 4 MiB
UTF-8 total. Sets live in memory; no read-ID file/index API is provided. Python
and the C options payload are capped at 16 MiB. There is no unlimited ID-file
loading or executable query language.

The complete ordered contig table is retained, including in empty outputs.
Coordinates, identity, filter_reason, read_length and query coordinates are
preserved. Writer rebuilds counts, shards and q1, whose threshold remains
**MAPQ >= 1**, independently of the selection threshold. All explicit `cn.info` declarations are retained with the full contig table,
even for empty results; missing CN remains missing. Result and provenance include
`copy_numbers_propagated`. Other application sidecars and index files are not copied.
See [CN rules](copy-numbers.md). This extracts core records;
it does not promise lossless extraction of arbitrary application metadata.

## Scan, memory and index policy

This version always sequentially scans q0, even for empty conditions and positive
MAPQ thresholds. It does not use QueryReader's q1 optimization. Region indexes
exist but provide no read locator; subset deliberately uses one conservative
sequential path for all modes. Existing, absent, stale or malformed index files
do not change results. `index_used=false` and `fallback_reason` explicitly
report this policy. There is no `index` option and no new index is built.

Memory depends on the largest decoded row group (including transient Polars and
owned column buffers), target input/output batches, Writer's chunk buffer and
encoding copies, and the largest individual read. Several copies of a read may
coexist. `batch_rows` is a target rather than an RSS cap; concat batches may
exceed it for one oversized read. Writer likewise allows an oversized read to
exceed chunk_size. Metadata/contig tables, the existing Reader shard inventory,
and bounded ID selection sets also occupy memory. No full record table or set
of all matching reads is accumulated. The entire input must remain unchanged;
there is no concurrent-mutation snapshot guarantee.

## Publication, provenance and errors

Metadata version/scope, conditions and output paths are checked before staging.
The parent directory must exist. Output and `.partial` must be absent, including
dangling symlinks. Resolved output/staging paths must not overlap the source or
its q0/q1 directories. Writer reserves staging and publishes without overwrite.
Normal read, field-validation, sidecar or write failures remove this operation's
staging directory; existing directories are preserved. Crash durability is only
that of the existing Writer protocol. Input data errors are not converted to
index fallback. Errors include source and available shard/field/read context.

With provenance enabled (default), `_subset.json` records source path label,
version/scope, conditions, effective modes, logical-ID relationship, actual
counts, scan/index status and omitted sidecars. Supplied IDs, including an empty
list, are written separately to `_subset_read_ids.jsonl`, one JSON string or
UInt64 per line. These are input **public logical IDs** for concat, not a claim
that remapped IDs equal original disk IDs. IDs are not inlined in an unbounded
manifest. All requested provenance is written/flushed before finish publishes.
With provenance disabled no subset sidecars are produced.

`SubsetResult.to_dict()` / Rust fields include absolute output path, format,
`counts` (q0/q1_records and q0/q1_concats), scanned_records, full_scan, index_used,
fallback_reason, provenance path (or null), and omitted_sidecars. Concat counts
count output reads, not matching alignments. `scanned_records` counts q0 records
traversed; successful calls always report full_scan=true. Counts do not trust
input metadata. No separate matching-alignment counter is exposed.

The additive C function `pqsio_subset_json(input, output, options_json, callback,
user)` shares the Rust engine; options use the names above. As with merge,
callback failure is reported *after* successful publication and does not remove
a valid result. Python detects a missing symbol in older native libraries and
reports a clear capability error; existing APIs remain available.

## Validation

Run in the project Pixi environment:

```sh
pixi run build
pixi run python -m unittest discover -s tests -p test_subset.py -v
pixi run cargo test --locked --profile dev-release --test subset
pixi run cargo test --locked --profile dev-release --lib subset::tests
```

Small synthetic tests compare every output field, q0/q1 order and logical ID to
an independent row oracle, then full-validate outputs. Tests cover both modes,
coordinate boundaries, read/row-group/shard boundaries, local IDs, empty inputs,
condition intersections, repeated IDs/records, provenance, paths and cleanup.
No large genome pipeline or memory benchmark is required or run.

The implementation was checked with `pixi run build`, `pixi run test-rust`,
`pixi run test`, `pixi run test-subset` and `pixi run lint` (dev-release).
All passed. The final subset suite has 10 Python test groups with many oracle
combinations and three new Rust tests (public API, bounded/non-executing options,
and provenance failure cleanup). Existing standalone Rust, Python, C and C++
regressions passed. A 270-alignment single-read fixture stays in one output
shard at batch targets 1, 64 and 1024; a late corrupt shard removes already
written staging output. Input hashes, q1 reconstruction and full output
validation are checked. No RSS measurement, sibling CPhasing integration suite,
crash-durability test or non-Linux platform validation was performed.

## Changed files

| Area | Files |
| --- | --- |
| Native subset and exports | `src/subset.rs`, `src/lib.rs`, `src/ffi.rs`, `include/pqsio.h` |
| Shared predicates, safe JSON literals, error context | `src/query.rs`, `src/metadata.rs`, `src/streaming.rs` |
| Python API | `python/pqsio/subset.py`, `python/pqsio/__init__.py` |
| Tests and task registration | `tests/subset.rs`, `tests/test_subset.py`, `pixi.toml` |
| Documentation | `README.md`, `docs/subset.md` |

No dependency declarations, lockfiles or disk-format templates were changed.
