# Streaming reads — row and column batches (0.0.6–0.0.7)

All APIs below are **new**, additive capabilities, not features of previous
pqsio releases. Existing Rust Reader, C reader functions, C++ Reader and Python
Reader.iter_batches()/iter_reads()/iter_columns() keep their previous behavior.
No disk schema, coordinate convention, Polars version, or runtime dependency changes.

## Options and semantics

`batch_rows` defaults to 65536, range **1..=4294967295**, in all bindings. Rust
uses usize, C/C++ use uint64_t at entry (validated before allocation), and Python
requires an integer in that range. min_mapq is 0..255 (u8 in native interfaces).
The default boundary is `rows`. An omitted concat filter means
`matching_alignments`. Python uses None, Rust uses None, C uses 0, and C++ uses
ConcatFilter::Default to represent omission. Pairs requires `rows` and an
omitted concat filter; even explicitly selecting matching_alignments is an error.
Unknown boundary/filter values are errors. Native unsigned parameters cannot
represent negatives; Python rejects negatives before calling native code.

| Boundary | Filter | Returned records and grouping |
|---|---|---|
| rows | matching_alignments | Only MAPQ-matching alignments; at most batch_rows; reads may split |
| complete_reads | matching_alignments | All **retained** alignments of each read together; not the original full read |
| rows | complete_reads | All q0 alignments of reads with any matching alignment; reads may split |
| complete_reads | complete_reads | All q0 alignments of qualifying reads together |

Rows batches pack across shards; every nonfinal batch is filled. Complete-read
batches greedily pack whole reads up to the target. A read exceeding the target
occupies its own oversized batch. A shard boundary never ends a global-ID read.
No empty output batches are delivered: Rust None / C status 0 / Python iterator
termination means EOF. MAPQ=0 retains every record/read. Entirely unmatched reads
are skipped. Both new filtering modes read q0, never reconstruct a read from q1.

## Examples

New Python API (legacy Reader remains available):

```python
from pqsio import StreamingReader
with StreamingReader("sample.concat.pqs", min_mapq=30, batch_rows=4096,
                     boundary="complete_reads", filter_mode="complete_reads") as reader:
    for batch in reader.iter_batches():
        consume(batch)  # possibly multiple complete reads
```

`StreamingReader.iter_columns()` additionally yields the existing `PairColumns`
or `ConcatColumns` types. Its options, grouping, filtering and EOF behavior are
identical to `iter_batches()`:

```python
with StreamingReader("sample.concat.pqs", min_mapq=30, batch_rows=4096,
                     boundary="complete_reads", filter_mode="complete_reads") as reader:
    for batch in reader.iter_columns():
        print(batch.read_offsets)  # UInt64 offsets into the alignment columns
        positions = memoryview(batch.start)
# positions still owns a reference to its Python array after the reader closes.
```

Each numeric column is copied once into a Python-owned array, using the existing
column-batch copier; no intermediate Python row objects or native row vector is
used by column output. Strings use UInt64 offsets + UTF-8 bytes as documented in
[columnar.md](columnar.md). Empty strings and long coordinates are preserved.
`iter_reads()` remains unsupported; select a boundary explicitly instead.

Row and column iteration share **one cursor** and may be interleaved: switching
representation consumes the next batch, without restarting or changing options.
For `rows` boundary, concat `read_offsets` group fragments **within this batch**;
a read can continue in the next batch. Only `complete_reads` boundary guarantees
that all returned alignments of a read stay together. `matching_alignments`
filtering still excludes low-MAPQ alignments even with complete-read boundaries.
No empty batches are emitted; an empty/all-filtered stream terminates normally.

A library exporting the original streaming API but not `pqsio_stream_next_columns`
continues to support streaming row batches. Column iteration checks this symbol
and raises a capability error before consuming input; it never falls back to rows.
With a pre-streaming library, the constructor still reports missing streaming
capability. Legacy Reader methods remain usable. Python records/arrays may outlive
the reader; keeping batches increases caller memory usage.

New Rust API:

```rust
use pqsio::{StreamingReader, ReadOptions, ReadBoundary, ConcatFilter};
let mut reader = StreamingReader::open("sample.concat.pqs", 30, ReadOptions {
    batch_rows: 4096,
    boundary: ReadBoundary::CompleteReads,
    concat_filter: Some(ConcatFilter::CompleteReads),
})?;
while let Some(batch) = reader.next_batch()? {
    consume(batch);
}
```

For Rust columns, replace `next_batch()` above with `next_columns()`. It returns
`Result<Option<ColumnBatch>>`, reusing the owned `Pairs` / `Concat` variants and
their Vec buffers; output can outlive the stream.

New C API (independent opaque pqsio_stream; no old ABI layout/signature changes):

```c
pqsio_stream *reader = NULL;
/* boundary 0=rows, 1=complete_reads;
   filter 0=omitted, 1=matching_alignments, 2=complete_reads */
if (pqsio_stream_open(path, 30, 4096, 1, 2, &reader) != 0) { /* handle error */ }
int32_t status;
while ((status = pqsio_stream_next(reader, NULL, consume_concat, user)) == 1) {}
/* status == -1: inspect pqsio_last_error() immediately */
pqsio_stream_destroy(reader);
```

C column output uses the existing independently owned batch handle:

```c
pqsio_column_batch *batch = NULL;
while ((status = pqsio_stream_next_columns(reader, &batch)) == 1) {
    pqsio_concat_columns view;
    if (pqsio_column_batch_concat(batch, &view) == 0) { /* consume view */ }
    pqsio_column_batch_destroy(batch);
}
```

The return status is 1 / 0 / -1; output is NULL on EOF/error. The output pointer
must be writable/aligned and cannot overwrite a live batch. Getters borrow immutable
buffers until batch destruction; closing/failing the stream does not invalidate
previously returned batches. Resolve the new symbol when loading older libraries.
No ABI version, old signatures, descriptor layouts or columnar version change.

New C++17 API:

```cpp
pqsio::StreamingReader reader(path, 30, 4096,
    pqsio::ReadBoundary::CompleteReads, pqsio::ConcatFilter::CompleteReads);
while (reader.next(nullptr, consume_concat, user)) {}
reader.close(); // optional early close; destructor also releases the handle
```

C++ `StreamingReader::next_columns()` returns the existing move-only RAII
`ColumnBatch`; `while (auto batch = reader.next_columns()) { auto view = batch.concat(); }`
uses the same ownership rules as `Reader::next_columns()`. A view must not outlive
its owning batch, but the batch can outlive the stream.

New pqsio_stream_kind/contigs and C++ kind()/contigs() expose dataset information.
C/C++ callbacks borrow arrays and strings only during the call, must return zero
on success, and must not throw/unwind, reenter, close, or share the handle across
concurrent calls. Copy anything retained. Rust batches own their rows. No native
pointer escapes Python callbacks. C destruction consumes a live handle (or accepts
NULL); never reuse a destroyed pointer. Python/C++ close is idempotent.

Any next_batch/next_columns/stream_next/stream_next_columns error, including a
failed C callback or NULL column-output argument on a live stream, poisons the
stream, releases decode/read buffers and file handles, and makes further reads
fail until close/reopen. The failing batch is not resumable and no damaged shard
is silently skipped. Previously delivered batches remain valid. Drop/close also
releases resources on early termination; Python generator locals or caller-owned
batches remain alive until those references are released.

## IDs, order, and layout assumptions

The new reader reuses Reader::open's metadata, contig table and shard ordering
(numeric stems first, numeric order, then other names). It shares the legacy
DataFrame-to-column conversion code, including wide coordinates and
categorical/string decoding. Both streaming representations share columnar decode,
ID mapping, filtering and batch assembly. Rows are built only for final row output.

For shard-local IDs, consecutive raw IDs are mapped to increasing global IDs
starting at 1 **before filtering**, and mapping state is reset only at a shard
boundary. Equal local IDs in different shards never merge. Removed reads still
consume their mapped ID; options never renumber subsequent reads. Global IDs
remain unchanged and may continue through adjacent shards, empty shards and
row-group boundaries.

The supported streaming layout requires nondecreasing IDs: global across all
shards, local within each shard. Each read is a consecutive run. The old reader
only grouped adjacent equal IDs and did not validate decreasing/reappearing IDs;
the new reader explicitly rejects such unsupported layouts instead of sorting or
keeping a dataset-wide seen-ID cache. Old methods still retain their permissive
behavior. There is no new global sorting, read-ID index or full-dataset cache.

## Decoder capability audit and memory accounting

Inspected the installed **polars-io 0.49.1** source, specifically
`src/parquet/read/reader.rs` (get_metadata, set_metadata, finish) and
`src/parquet/read/read_impl.rs` (read_parquet, rg_to_dfs and its serial path).
The public ParquetReader has metadata and slice APIs but no public batched cursor.
Plain repeated slices go through a row-group traversal; this implementation does
not use that approach. It reads metadata once per shard and passes metadata with
**exactly one original row group**, its original absolute column byte offsets,
and the corresponding row count to a serial decoder. Each row group is decoded
once. No complete shard DataFrame or repeated prefix/full-shard decode occurs.
Multi-row-group fixtures verify that offsets, fields and ordering are preserved.

Memory consists of:

* Decoder: one row group's columns plus Parquet dictionary/page decompression
  buffers. Polars may mmap the entire file's virtual address range; this is not
  a heap copy or decoding of the entire shard. Its mapping is released after
  conversion. Resident page/cache behavior depends on the OS.
* Conversion: the row-group DataFrame and owned typed column vectors temporarily
  overlap. Exhausted column buffers are released before decoding the next group.
  UTF-8 and numeric fields are copied into continuous owned buffers.
* Output: retained contiguous spans are copied into an owned column batch, up to
  batch_rows except an oversized complete read. C/C++ borrow the resulting buffers
  through existing batch getters; Python bulk-copies each column into an initialized
  array. Final row output instead allocates row structs/strings; its C ABI then
  creates callback descriptors/C strings and Python record objects. Thus the shared
  column implementation also changes row-path allocation costs.
* Read staging: rows+matching_alignments needs no full-read staging. Other
  combinations accumulate a raw complete read across groups/shards before filtering;
  matching-alignments filtering copies retained spans to another column buffer.
  A ready read and the current batch can overlap decoder columns containing the
  start of the next read. Fully rejected very large reads can still require
  substantial temporary memory. Complete-read filtering with rows buffers a full read.
* Metadata: shard path list, current file footer/row-group metadata and a temporary
  metadata clone. These scale with shard count / row-group count, not decoded
  shard rows. Metadata cloning costs grow with the number of row groups.

**batch_rows is output row control, not a strict byte or RSS memory limit.**
A shard consisting of one huge row group still requires that decoding unit;
a huge read requires read-sized staging/output where applicable. Changing an
existing file's row-group layout is outside this change. No async prefetch,
parallel reader, remote storage, format change or new dependency is introduced.

The earlier [row-only measurements](streaming-results.md) describe the 0.0.6
implementation before shared columnar staging; they are not measurements of this
extension. No new throughput/zero-copy claim is made. Focused validation uses
`pixi run test-streaming` and `pixi run test`: cross-representation equality and
batch boundaries, all filter/boundary combinations, multi-group/shard fixtures,
legacy IDs, empty/all-filtered streams, UTF-8/empty strings, wide coordinates,
terminal failures, interleaved calls and C/C++/Python ownership.

## Column-output validation and changed files

Validated through the local Pixi environment and `dev-release`: `pixi run test`
passed 18 Rust tests and 38 Python/C/C++ tests; `pixi run lint` and
`git diff --check` passed. The expanded `test-streaming` suite has 11 tests,
including an independent legacy Reader q0 field oracle and compiled C11/C++17
column consumers. No new performance measurement, aarch64 run or large dataset
pipeline was performed for this extension.

Changed files: `src/streaming.rs`, `src/columns.rs`, `src/ffi.rs`,
`include/pqsio.h`, `include/pqsio.hpp`, `python/pqsio/__init__.py`,
`python/pqsio/columns.py`, `tests/storage.rs`, `tests/test_streaming.py`,
`tests/streaming.c`, `tests/streaming.cpp`, `README.md`, and `docs/streaming.md`.
No dependency, lockfile, disk-format or existing ABI-signature changes.
