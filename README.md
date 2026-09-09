# pqsio

Reusable Rust storage for **pairs PQS 0.1.0** and **alignment-level concat PQS
0.2.0**, with a C ABI, C++17 convenience wrappers and a dependency-free Python
binding. This is a local development project; the package name has not been
reserved on PyPI or crates.io.

The library is independent of CPhasing's phasing/alignment algorithms. It uses
the same Polars 0.49.1 Parquet implementation and metadata conventions as the
surrounding project. Existing CPhasing/Chromap commands are not modified.

## Build in this workspace

From `/data3/wangyb/0.CPhasing/v4.0`:

```sh
pixi run --manifest-path cphasing-rs/pixi.toml cargo build --offline --manifest-path pqsio/Cargo.toml --profile dev-release -j 4
pixi run --manifest-path cphasing-rs/pixi.toml cargo test --offline --manifest-path pqsio/Cargo.toml --profile dev-release -j 4
```

The artifacts are `pqsio/target/dev-release/libpqsio.so` (Linux), `libpqsio.a`,
and an ordinary Rust library. The new Cargo.lock starts from the surrounding
project's dependency versions; no parent lockfile is changed. Outside this
workspace, use Cargo directly with a compatible toolchain (tested with Rust 1.91.1).
First-time builds require the dependencies to be cached or network access.

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
`writer_open`. Strands are the ASCII bytes `+` and `-`. ABI v1 uses an array of
records for simplicity; a column-buffer/Arrow interface can be added later
without changing the on-disk schema.

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
export PQSIO_LIBRARY="$PWD/pqsio/target/dev-release/libpqsio.so"
PYTHONPATH=pqsio/python python -m unittest discover -s pqsio/tests -p test_python.py -v
# Polars/CPhasing are test-only dependencies for compatibility checks:
PYTHONPATH=pqsio/python:CPhasing python -m unittest discover -s pqsio/tests -p 'test_*.py' -v
```

Rust tests cover long positions, MAPQ filtering, complete/oversized concat
reads, invalid input, abort cleanup and output collisions. Python/C/C++ tests
exercise the actual shared library. Small independent Polars fixtures test
legacy shard layouts and schema compatibility.

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
