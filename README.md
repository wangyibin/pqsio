# pqsio

Reusable Rust storage for **pairs PQS 0.1.0** and **alignment-level concat PQS
0.2.0**, with a C ABI, C++17 convenience wrappers and a dependency-free Python
binding. This is a local development project; the package name has not been
reserved on PyPI or crates.io.

The library is independent of CPhasing's phasing/alignment algorithms. It uses
the same Polars 0.49.1 Parquet implementation and metadata conventions as the
surrounding project. Existing CPhasing/Chromap commands are not modified.

## Pixi development environment

From the `pqsio` repository directory:

```sh
pixi install --locked
pixi run build          # dev-release; native shared/static/Rust libraries
pixi run test           # standalone Rust, Python, C11 and C++17 tests
pixi run lint           # strict Clippy, using dev-release
```

`pixi.toml` manages the Rust 1.91 toolchain, C/C++ compilers and build tools,
plus Python 3.11 and Polars 0.20.29 for tests only. Rust library dependencies
remain in `Cargo.toml` / `Cargo.lock`, including Polars 0.49.1. The base Python
package still has no third-party runtime dependencies. `pixi.lock` pins the
development environments; `.pixi/` is local and ignored by Git.

Pixi sets `PYTHONPATH` and `PQSIO_LIBRARY` for the local package and
`target/dev-release/libpqsio.so`, plus four Polars/Rayon threads. Use
`pixi run python` for an interpreter configured to use the development library.
The standalone test task does not require the sibling CPhasing checkout.

Individual tasks are `test-rust`, `test-python`, `test-columns`, `test-native`,
`test-parallel`, `test-streaming`, `test-query`, `test-inspection`, `test-merge`
and `test-subset`. Python/native tests build the shared library first. `pixi run bench-columns --rows
80000 --repetitions 5`, `pixi run bench-streaming` and `pixi run bench-query`
explicitly run synthetic benchmarks; normal builds/tests do not
run them. `pixi run build-release` is reserved
for final release artifacts under `target/release/`; development and debug work
use `dev-release`. To use release libraries in Python, explicitly override
`PQSIO_LIBRARY` after activation.

From the parent workspace, use `pixi run --manifest-path pqsio/pixi.toml build`.
The manifest targets Linux x86-64 and aarch64; platform validation is documented
below. First installation/build needs cached packages or network access.
Cargo tasks use `--locked`; add `CARGO_NET_OFFLINE=true` when all Rust crates are
cached and offline operation is required. Use `pixi install --locked` to verify
reproducibility; run `pixi lock` deliberately when changing development dependencies.

## Metadata and read-only validation

`pqsio.inspect(path)` returns structured metadata and observed Parquet footer
information. `pqsio.validate(path, level="quick" | "full")` returns a diagnostic
report distinguishing invalid data from incomplete checks. Rust APIs and additive
C JSON callbacks share the implementation. See [contracts and limits](docs/inspection.md).
Run `pixi run test-inspection` for focused tests.

## Streaming subset

`pqsio.subset(input, output, regions=[("chr1", 0, 1000)], min_mapq=30)`
exports selected q0 records through native streaming columns, preserving fields,
contigs, order and public logical IDs. Concat additionally supports
`mode="complete_reads"`. Writer rebuilds q1 and counts; provenance is optional.
See [subset API, semantics and memory limits](docs/subset.md).

## Streaming merge

`pqsio.merge(inputs, output, chunk_size=1_000_000, batch_rows=65_536,
provenance=True)` merges caller-ordered pairs or concat datasets through the
native column pipeline. It unions contigs, preserves pairs IDs, assigns global
concat IDs, and rebuilds q1 from q0. Application sidecars (including `cn.info`)
are not propagated. See [merge contracts, provenance and memory limits](docs/merge.md).

## Rust

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

## Streaming reads (row API: 0.0.6; column API: 0.0.7)

New `StreamingReader` APIs in Rust, C, C++ and Python decouple output batches
from disk shards. `batch_rows`, read-boundary policy and concat MAPQ filtering
are independent options. Complete-read filtering returns qualifying reads from
q0 including low-MAPQ alignments. `StreamingReader.iter_columns()` (Rust/C++:
`next_columns`, C: `pqsio_stream_next_columns`) now returns owned column batches
with the same options and cursor, without intermediate row conversion. Existing
`Reader` methods are unchanged.
See [new interfaces, semantics and memory limits](docs/streaming.md) and
[validation and measurements](docs/streaming-results.md).

## Optional region queries and row-group summaries

`QueryReader(path, regions=[("chr1", 100_000, 200_000)], index="auto")` queries
through the shared streaming column pipeline. Positive-MAPQ matching queries
select q1 independently of index mode; `auto/off/require` target that partition.
`stats["source_quality"]` identifies the source. `build_index(path)` builds q0;
`build_index(path, quality="q1")` builds an independent q1 index. Both add an
optional per-contig/per-endpoint row-group summary sidecar; `index="off"` is the
sequential path for the selected partition. Concat complete-read queries and legacy
shard-local IDs always use sequential scans. No dependencies or source PQS
files change. See [API, schema, fallback and memory contracts](docs/query.md)
and [synthetic measurements](docs/query-results.md).

## Columnar batch I/O (0.0.4)

Synchronous writers also accept `PairColumns` / `ConcatColumns` through Python
`write_columns`, Rust typed slice views, and additive C/C++ buffer descriptors.
`Reader.iter_columns()` yields independent Python array-backed batches; no
intermediate row objects are constructed. The base package remains standard
library only. See [types, examples, ownership and capability detection](docs/columnar.md)
and [measured performance and copy costs](docs/columnar-results.md).

## Parallel writing (0.0.3)

`ParallelWriter` and shareable `Producer` interfaces are available in Rust,
Python, C and C++17. They provide an ordered bounded input queue, concurrent
shard encoding, and final publication after all workers succeed. Batch sequence
numbers start at zero; concat batches must contain complete reads. Existing
synchronous handles remain non-thread-safe.

See [parallel API, memory limits and examples](docs/parallel.md) and
[acceptance results](docs/parallel-acceptance.md). Batch submission is asynchronous:
`finish()` is required to observe all errors. Each batch ends a shard.

## Python

Use `PYTHONPATH=pqsio/python` from the workspace root, or install this directory
as a Python package. The Python package does **not** compile or bundle the
native library; set `PQSIO_LIBRARY` to the absolute path of the built `.so`.
There is no Python runtime dependency beyond the standard library.

```python
from pqsio import Pair, Alignment, PairsWriter, ConcatWriter, Reader

with PairsWriter("sample.pairs.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_batch([Pair("read1", 0, 10, 0, 200, "+", "-", 60)])

with ConcatWriter("sample.concat.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_read([
        Alignment(1, 200, 0, 100, "+", 0, 10, 110, 60, 0.99),
        Alignment(1, 200, 100, 200, "-", 0, 500, 600, 20, 0.98),
    ])

with Reader("sample.concat.pqs", min_mapq=1) as reader:
    print(reader.kind, reader.contigs)
    for alignments in reader.iter_reads():
        print(alignments)
```

`PairsReader` and `ConcatReader` additionally check the dataset kind.
`iter_batches()` yields one Parquet shard at a time. Python objects are copies;
no borrowed native pointer escapes into the public API.

### Bulk concat writes (0.0.2)

```python
# Every inner list contains a complete read; read IDs increase across calls.
with ConcatWriter("bulk.concat.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_reads([
        [Alignment(1, 100, 0, 50, "+", 0, 10, 60, 30, 0.99)],
        [Alignment(2, 100, 0, 50, "-", 0, 100, 150, 60, 0.98)],
    ])
# Alternatively: writer.write_batch(flat_alignments, [0, end_read1, end_read2, ...])
```

The caller controls batch size; `write_reads` materializes its iterable.
All offsets and reads are validated before accepting a batch. Empty batches
use offsets `[0]`; empty reads are rejected. Input validation errors accept no
records from that batch; storage failures still poison/abort the staged output.

The new Python package remains usable with an old ABI v1 library for existing
methods; bulk writes report that a >=0.0.2 library is required. New C/C++ callers
use `pqsio_write_reads` / `Writer::write_reads(rows, offsets)`.

Bulk submission reduces native call overhead. It is not automatically faster
for Python dataclass inputs: fixed-field encoding already improves the existing
single-read method. See the measured tradeoffs in the acceptance report.

## C and C++

Include `include/pqsio.h` (C) or `include/pqsio.hpp` (C++17). Link with
`-L/path/to/pqsio/target/dev-release -lpqsio` and configure the shared-library
search path, for example with `-Wl,-rpath,/path/to/pqsio/target/dev-release`.
The executable examples in `tests/smoke.c` and `tests/smoke.cpp` cover both
formats and both reading and writing.

The ABI accepts structured arrays, not serialized text. Strings are UTF-8,
NUL-terminated. Numeric contig IDs index the ordered contig table supplied to
`writer_open`. Strands are the ASCII bytes `+` and `-`. ABI v1 retains its array-of-records functions. The additive columnar extension
uses typed spans and packed UTF-8 offsets without changing the disk schema.

Writes consume/copy inputs before returning. Reader callbacks borrow arrays
and strings only during the callback; copy any data retained afterwards.
Callbacks must return 0 for success and must not unwind or throw across the C
boundary. All handles and pointer/length pairs must remain valid; a stale or
otherwise invalid foreign pointer cannot be validated by Rust. A handle must
not be used concurrently or re-entered from its callback.

`pqsio_reader_next` returns 1 for a delivered shard, 0 for EOF, -1 on error.
Other status functions return 0 or -1, except `reader_kind` (0 pairs/1 concat).
Read `pqsio_last_error()` immediately after failure: the error is thread-local
and the next API call may replace it. Rust panics are caught at the ABI boundary.
A read error or failed callback may consume the current shard; abort/reopen the
reader after an error. Explicitly finish writers; destruction only cleans up.

## Storage and compatibility contracts

- Both datasets have `_contigsizes`, `_metadata`, `_metadata_counts`, `_readme`,
  `q0/` and `q1/`. q0 contains **all** records; q1 contains MAPQ >= 1, not a
  disjoint complement. Concat uses `mapping_quality`; pairs uses `mapq`.
- pairs positions are **1-based**. concat reference/query intervals are
  **0-based, half-open**. No implicit coordinate conversion is performed.
- pairs positions use UInt32 when contig lengths permit, otherwise UInt64.
  concat reference positions use UInt64, compatible with the existing format;
  read length and read offsets remain UInt32.
- The metadata retains CPhasing's Python-dictionary representation, including
  Polars dtype names. The Rust reader does not execute the metadata as code.
- Writer calls validate before accepting the batch/read. A storage error marks
  the writer failed. Data is staged in `<output>.partial`, and a successful
  `finish` publishes the result. Existing outputs or staging directories are
  rejected. Drop/close aborts unfinished output. A process crash can leave a
  `.partial` directory; investigate/remove it before retrying. Publication uses
  same-filesystem rename, with a brief empty destination reservation to avoid
  replacing a concurrent creator; readers only recognize completed metadata.
  This is not an fsync-based crash-durability guarantee.
- Empty datasets are supported and have no Parquet shards. A concat read larger
  than the target shard size occupies one oversized shard. Other complete reads
  are packed without crossing shard boundaries.
- min_mapq > 0 yields only retained alignments, not the original full read.
  When reading existing concat metadata with `read_idx_scope='shard'`, the
  reader scans q0 and assigns deterministic increasing global IDs before
  filtering, keeping separate shards' equal IDs separate. Global IDs are kept
  unchanged. Read order follows numeric shard names, then other names.
- This version preserves the core records/schema. It does not automatically
  copy optional application files such as `cn.info`; it is not a general
  lossless clone operation for all dataset sidecars.

## Verification

```sh
# From pqsio; these tasks configure paths and build the development library:
pixi run test
pixi run lint
```

Rust and standalone Python/C/C++ tests cover row/column cross-path I/O, long
positions, MAPQ filtering, complete/oversized concat reads, invalid input,
buffer lifetimes, abort cleanup and output collisions. Independent Polars
fixtures in the column tests cover legacy shard-local read IDs and disk schemas.
The historical-library test skips when its named archived v0.0.1 library is
absent from a fresh checkout.

The standalone Pixi environment was validated on Linux x86-64 with Rust 1.91.1
and Python 3.11.16: 16 Rust tests and 27 Python/native tests passed. Native tests
honor activated `CC`/`CXX` (falling back to `cc`/`c++` outside Pixi). The aarch64
environment is resolved in the lockfile but has not been built or run here.

The additional `tests/test_compatibility.py` integration suite requires a
separately configured full CPhasing environment (its Python dependencies and
optionally its Rust executable), rather than the standalone Pixi environment.
Run it from the parent workspace with that environment's Python:

```sh
PQSIO_LIBRARY="$PWD/pqsio/target/dev-release/libpqsio.so" PYTHONPATH=pqsio/python:CPhasing python -m unittest discover -s pqsio/tests -p test_compatibility.py -v
```


## Current scope

The current implementation uses synchronous Parquet writes with bounded shard
buffering and normal backpressure. It does not yet add asynchronous writer
queues, zero-copy Python arrays, projection/predicate pushdown, a text CLI,
Chromap output flags, or replace CPhasing's existing readers/writers. These can
be integrated using the library without changing the format. No whole-genome
performance claim is made; Chromap integration and throughput measurements
remain separate follow-up work.

## Polars version comparison

An isolated 0.49.1 versus 0.55.2 comparison lives in
[comparison/README.md](comparison/README.md). It does not upgrade this package's
production dependency. See that directory for reproduction commands and results.

## v0.0.2 optimization acceptance

See [benchmarks/ACCEPTANCE.md](benchmarks/ACCEPTANCE.md) for measured performance,
compatibility checks, limits, and reproducible commands against Git `v0.0.1`.

Public `StreamingReader` automatically reads q1 for positive `min_mapq` on pairs
and global-ID concat matching queries. MAPQ zero, complete-read filtering and
legacy shard-local concat use q0. Batch boundaries do not change this selection;
see [streaming semantics](docs/streaming.md).
