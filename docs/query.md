# Optional q0/q1 row-group region summaries — 0.0.10

This additive API leaves PQS 0.1.0 pairs / 0.2.0 concat files, the legacy APIs,
ABI v1, traversal order and output coordinates unchanged. It adds no runtime
dependency, database, CLI, read/alignment locator, parallel query or prefetch.
An index may reduce data decoding; it is not a promise of lower latency or
physical disk I/O.

```python
import pqsio

pqsio.build_index("sample.pqs")  # q0; existing calls remain compatible
pqsio.build_index("sample.pqs", quality="q1")  # independent index; explicit rebuild=True
with pqsio.QueryReader(
    "sample.pqs", regions=[("chr1", 100_000, 200_000)], min_mapq=30,
    index="auto", pairs_mode="either", batch_rows=65_536,
) as reader:
    for batch in reader.iter_columns():
        consume(batch)
    print(reader.stats)
# reader.stats remains available after close, including incomplete statistics.
```

Data source selection is automatic (Rust, Python and the C bridge share it):

| Condition | Data source |
|---|---|
| `min_mapq=0` | q0 |
| `min_mapq>=1`, pairs or global-ID concat matching, any index mode | q1 |
| concat `complete_reads` or shard-local ID scope | q0, sequential |

`reader.stats["source_quality"]` reports `"q0"` or `"q1"`. All group/row
counters describe that selected partition. q1 queries do not open q0 Parquet
files or its index. Source selection is independent of `index`: `auto` uses
the selected partition's valid index or scans that partition; `off` scans it;
`require` requires its valid index. In particular, **require no longer forces
q0** for positive-MAPQ matching queries. Neither quality's index substitutes for
the other. q1 is trusted under the existing PQS contract: it must be the complete
ordered MAPQ >= 1 view of q0, preserving duplicate rows and global IDs. Missing
or unreadable source files are errors, not index fallbacks. Building a q1 index
does not establish q0/q1 completeness by rescanning q0.
The public `StreamingReader` shares this automatic q0/q1 source selection, without region-index pruning.

For concat, `filter_mode="matching_alignments"` (also the default `None`) returns
only alignments satisfying region AND MAPQ. `filter_mode="complete_reads"`
returns all q0 alignments of each qualifying read, including low-MAPQ and
out-of-region alignments. It **always scans sequentially**. `boundary="rows"`
or `"complete_reads"` independently controls output batches; complete-read
boundaries apply to the returned alignments, as in StreamingReader. Pairs
requires `boundary="rows"` and `filter_mode=None`. `pairs_mode="both"` requires
pairs; concat uses the default `"either"` value, which has no concat effect.
`iter_batches()` is available through the same native column cursor. Old
native libraries retain their existing APIs; the two new Python entry points
check native symbols and report a specific rebuild/update capability error.

```rust
use pqsio::{build_index_for_quality, IndexQuality, QueryReader, QueryOptions, Region, IndexMode};
# fn example() -> anyhow::Result<()> {
build_index_for_quality("sample.pqs", IndexQuality::Q1, false)?;
let mut reader = QueryReader::open("sample.pqs", QueryOptions {
    regions: vec![Region { contig: "chr1".into(), start: 100_000, end: 200_000 }],
    min_mapq: 30,
    index: IndexMode::Require,
    ..Default::default()
})?;
while let Some(columns) = reader.next_columns()? { /* consume(columns) */ }
println!("{}", reader.stats().to_json());
# Ok(()) }
```

Rust additionally exposes `PairsMode`, `QueryStats`, and existing `ReadOptions`,
`ReadBoundary`, `ConcatFilter`. The additive C bridge exports
`pqsio_build_index`, `pqsio_build_index_quality`, `pqsio_query_open`,
`pqsio_query_stats_json`. The old Rust `build_index(path, rebuild)` and C
`pqsio_build_index(path, rebuild)` remain q0-only. The new C builder takes
`(path, quality, rebuild)` where quality is 0 or 1. Python checks the new symbol
for q1 builds and positive-MAPQ matching queries, giving an explicit capability
error on an older native library. A query handle
is a normal `pqsio_stream*`; use the existing stream column/row iteration,
metadata and destruction functions. No C++ wrapper or ABI version change is
needed to use the C bridge; no new C++ convenience API is supplied.

## Exact semantics and stable IDs

Every region names an existing contig and must satisfy
`0 <= start < end <= contig_length`, with u64 coordinates. Invalid regions are
errors before any output. Regions are sorted and merged per integer contig ID.
The union is applied once to each source row: overlapping regions cannot
multiply it, and genuinely repeated source rows survive. An empty region list
returns no rows; it can bypass data decoding even with `index="off"`.

Pairs tests `[pos-1, pos)` against regions, without changing emitted 1-based
positions. `either` accepts either endpoint; `both` requires each endpoint to
hit the union, possibly in different regions/contigs. MAPQ is also required.
Concat uses `alignment.start < query.end && alignment.end > query.start` and
MAPQ on the **same alignment**. Adjacent intervals do not overlap.

q0 and q1, index and off paths run the same Rust predicate inside StreamingReader's
columnar filtering/batching. Summaries only reject blocks. They cannot establish
that endpoints, coordinates or maximum MAPQ belong to the same source record.
For `both`, independent endpoint summaries may produce false positives, which
are removed by exact filtering. No result set or content-based deduplication is
used. Original shard order (numeric stems first, then lexical names) and row
order are retained.

Global concat read IDs pass through unchanged, including across group/shard
boundaries. Shard-local concat IDs retain the existing sequential mapping from
1, assigned before filtering; skipping prefixes cannot reproduce that mapping
without additional metadata. Such inputs **always scan sequentially**, as does
`complete_reads`. No per-read map or block read-count approximation is added.
The API assumes the same valid, ordered concat ID scopes as StreamingReader.

## Disk schema (version 1)

The optional `.pqsio-index/` directory is an application sidecar, not a change
to PQS metadata or Parquet. q0 retains its original root `.pqsio-index/`; q1
uses `.pqsio-index/q1/`. Each root independently contains:

- `CURRENT`: a short ASCII generation name `<process-id>-<timestamp-ns>`.
- `<generation>/manifest.json`: bounded to 16 MiB on disk.
- `<generation>/<shard-ordinal>.rg`: one little-endian binary partition per selected-quality
  shard, including an empty partition for a shard with no row groups.
- During construction only: `BUILD.lock`, `<generation>.partial/`, and a
  temporary CURRENT file.

Manifest fields:

| Field | Meaning |
|---|---|
| `version` | `pqsio-region-1` |
| `algorithm` | `contig-endpoint-envelope-1` |
| `coordinates` | `0-based-half-open; pairs=[pos-1,pos)` |
| `complete` | Integer `1`, written only after a successful build; not a JSON boolean |
| `source.format`, `source.format_version` | Source PQS format and version |
| `source.metadata_fnv1a64`, `contigs_fnv1a64`, `counts_fnv1a64` | Raw `_metadata`, `_contigsizes`, `_metadata_counts` content checksums; absent counts recorded as `"absent"` |
| `source.shards` | Ordered selected-quality shard inventory: name, size, mtime seconds/nanoseconds, footer checksum, row-group count |
| `build` | `quality="q0"` or `"q1"`, `partition="shard"`, `summary_cache_bytes=0` |
| `parts` | In shard order, each partition's byte length and whole-file FNV-1a checksum |

The binary layout and version remain unchanged; existing version-1 q0 indexes
with `build.quality="q0"` remain valid at their original location. q1 requires
`build.quality="q1"`; the reader rejects a manifest from the wrong quality even
when the two source inventories happen to be identical. Each quality has its
own CURRENT, generations, partition checksums and rebuild lifecycle. Shared
metadata/contig/count-file changes can invalidate both; a data-shard change
invalidates only the corresponding quality index.

Names are stored once in the inventory/contig table, never in each summary.
Source footer checksums cover the serialized Parquet footer plus its trailing
length/magic. FNV-1a-64 is a deterministic, **non-cryptographic** corruption and
identity check. The manifest uses the existing safe non-executing metadata
parser, with only the JSON subset that parser already accepts.

Each partition repeats a 32-byte row-group header followed immediately by its
summaries. The partition ordinal identifies its shard. Header fields are four
u64 values: `row_group`, `original_rows`, `shard_row_offset`, `summary_count`.
Rows and offsets describe the selected partition before query filtering. The summary location is the owning
partition at `header_file_offset + 32`; its extent is `summary_count * 32`.
The next header follows that extent. There is no in-memory row-group directory.

Each 32-byte summary contains:

| Byte offset | Type | Meaning |
|---|---|---|
| 0 | u32 | Contig ID, in `_contigsizes` order |
| 4 | u8 | Endpoint: 0 concat, 1 pairs first, 2 pairs second |
| 5 | u8 | Maximum MAPQ for this contig/endpoint in this row group |
| 6 | 2 bytes | Reserved, zero |
| 8 | u64 | Minimum 0-based start |
| 16 | u64 | Maximum exclusive end |
| 24 | 8 bytes | Reserved, zero |

Summaries are sorted by `(contig_id, endpoint)` inside each group. They never
merge coordinates of different contigs. Empty groups have zero rows, zero
summaries and the unchanged row offset. Nonempty groups must have summaries.
No alignment/read positions or read IDs are saved in the index.

## Building, identity and fallback

Footer metadata supplies row-group count, original row counts and cumulative
row offsets. It cannot in general provide per-contig, per-endpoint envelopes
and MAPQ: statistics may be absent, cover mixed contigs, or lack the necessary
relationships. Construction therefore decodes **each selected-quality row group**, using the
existing Rust column conversion (including its null/type/contig checks). It
scans typed buffers in Rust, builds only the current group's summaries, then
writes and releases them. No Python row objects, whole-shard DataFrame or
whole-dataset alignment/read list is used. Coordinates must have positive
length and fit the named contig; pairs position zero is rejected. Index build
is not a replacement for `validate(level="full")` and does not certify every
unrelated source contract such as global read ordering.

Selected-quality source identities and inventory are captured before and after construction.
A detected change aborts publication. Separate per-quality build locks prevent overlapping builds of the same
quality; a q0 build lock does not block a q1 build. Files are built in a unique temporary generation, flushed/synced, then
renamed; CURRENT is replaced by one final atomic rename. `rebuild=False` never
silently replaces an existing CURRENT. Normal failures remove this attempt's
temporary files and retain the prior published generation. No source file is
written. Existing `inspect`/`validate` continue inspecting source PQS data and
footers; they do not certify this optional index. Query diagnostics report
whether it passed index validation.

At query open, source parsing/footer errors propagate directly. Only then does
index validation run when an index is eligible, checking versions, completion,
`build.quality`, source identity, partition
length/checksum and structural group headers. All partitions are preflighted
**before any output**, using sequential bounded buffers. This deliberately
trades index-open I/O for reliable fallback without duplicate results. During
query, one shard partition is read sequentially with row groups. All shards
may contain requested contigs, so this first implementation does not eliminate
partitions with a separate contig-to-shard directory. It never loads all
summaries or candidate blocks into a Python dictionary or Rust set.

- `auto`: a missing, stale, corrupt, incomplete, wrong-quality or unsupported
  index falls back to sequential reading of the **selected** quality;
  `fallback_reason` explains why.
- `off`: does not open any index file. It still inspects source footers to
  report total row groups and source inventory information for the selected partition.
- `require`: the selected quality's unavailable index is an actionable error. A valid index is still
  **not used for acceleration** for complete reads or shard-local concat;
  the statistics explicitly explain this mandatory sequential behavior.
- Once iteration has begun, a later index/source error is terminal and poisons
  the stream. No restart or fallback can repeat previously returned rows.

The data must remain immutable during and after index building and throughout
querying. Size, mtime and footer checks cannot prove that arbitrary data-page
changes preserving file attributes will be detected. There is no adversarial
integrity guarantee or snapshot isolation for concurrent mutation. Filesystem
cache effects and physical bytes read are not inferred from file lengths.

Old generations intentionally remain available to already-open readers after
rebuild. After all readers/builders are closed, the entire `.pqsio-index/` can
be deleted and rebuilt, or unused generations removed manually. Deleting only
`.pqsio-index/q1/` removes q1 indexes without affecting q0. Crash leftovers
(including BUILD.lock) need manual cleanup only after confirming no build is
active; power-loss durability and crash-recovery automation are not promised.

## Memory accounting and diagnostics

| Component | Actual control / limitation |
|---|---|
| Build summary buffer | One row group's BTreeMap, at most 2 entries per contig for pairs, 1 for concat; allocator overhead is not byte-capped |
| Build file output | 64 KiB BufWriter per active partition; checksum pass uses a 64 KiB buffer |
| Query summary load | One 64 KiB BufReader and one 32-byte summary; preflight similarly streams partitions |
| Summary cache | **0 bytes**, always disabled; no positive-size cache option in this version |
| Candidate list | None; two endpoint booleans for the current group; counters only |
| Query regions | Caller-supplied regions and a sorted/merged per-contig interval table, O(number of regions); not part of the summary cache |
| Source/manifest inventory | O(shards + filename bytes), plus contig table; existing Reader retains its shard paths. Manifest parsing/building temporarily expands JSON into strings/values and may hold copies. 16 MiB is a serialized manifest limit, **not a heap-byte limit** |
| Footer metadata | Existing Polars metadata for one shard, proportional to that shard's groups/columns; not bounded by batch_rows |
| Decode | One entire row group, plus DataFrame-to-owned-column conversion temporaries; not a strict byte cap |
| Read/output | Existing stream batching: batch_rows rows, UTF-8 bytes, and potentially a complete oversized read; complete-read modes may span many groups/shards and exceed batch_rows |
| Python consumption | Native output and independent Python array copies coexist during transfer. Retaining batches is caller-controlled |

`stats` fields are:

- `index_used`: the index pruning path was selected, not a claim of speedup.
- `fallback_reason`: disabled/unavailable index or required semantic fallback.
- `source_quality`: `"q0"` or `"q1"`, the selected source partition.
- `total_row_groups`: selected partition footer count, known at open.
- `candidate_row_groups`: candidates **encountered so far**; off counts visited
  groups except the empty-region shortcut. No global candidate list is built.
- `decoded_row_groups`, `decoded_rows`: actual successful data-frame decoding;
  footer/summary reads do not count. Empty groups need no decoding.
- `skipped_row_groups`: groups excluded so far; candidates + skipped equals
  total only after complete consumption. Empty-region queries can skip without
  an index; indexed empty groups are also skipped.
- `returned_rows`: rows produced by the native iterator, not confirmation that
  downstream code successfully consumed or persisted them.
- `complete`: true only when next returns EOF; early close/error leaves false.
- `manifest_bytes`: validated manifest's on-disk size, zero when not loaded.
- `source_inventory_bytes`: serialized source identity size, a reproducible
  inventory-size indicator, **not an estimate of actual resident heap bytes**.
- `summary_cache_bytes`: zero, separate from manifest/inventory and IO buffers.

There is no read-byte metric because this backend does not expose a trustworthy
physical I/O count. Unusually large row groups, very large contig/shard tables,
or a single huge complete read remain memory risks inherited from the current
reader. The row-group summary design adds no unbounded candidate collection or
persistent summary cache; it does not impose a strict whole-process byte limit.
