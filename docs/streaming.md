# Streaming row reads — 0.0.6

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

StreamingReader offers row batches only. Its iter_reads and iter_columns methods
raise NotImplementedError to avoid inheriting ambiguous semantics. With an old
native library, its constructor reports missing streaming capability; old Reader
methods remain usable. Python records are owned copies, including strings, and
may outlive the reader. Keeping batches increases caller memory usage.

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

New C++17 API:

```cpp
pqsio::StreamingReader reader(path, 30, 4096,
    pqsio::ReadBoundary::CompleteReads, pqsio::ConcatFilter::CompleteReads);
while (reader.next(nullptr, consume_concat, user)) {}
reader.close(); // optional early close; destructor also releases the handle
```

New pqsio_stream_kind/contigs and C++ kind()/contigs() expose dataset information.
C/C++ callbacks borrow arrays and strings only during the call, must return zero
on success, and must not throw/unwind, reenter, close, or share the handle across
concurrent calls. Copy anything retained. Rust batches own their rows. No native
pointer escapes Python callbacks. C destruction consumes a live handle (or accepts
NULL); never reuse a destroyed pointer. Python/C++ close is idempotent.

Any new next_batch/stream_next error, including a failed C callback, poisons the
stream, releases decode/read buffers and file handles, and makes further reads
fail until close/reopen. The failing batch is not resumable and no damaged shard
is silently skipped. Previously delivered batches remain valid. Drop/close also
releases resources on early termination; Python generator locals or caller-owned
batches remain alive until those references are released.

## IDs, order, and layout assumptions

The new reader reuses Reader::open's metadata, contig table and shard ordering
(numeric stems first, numeric order, then other names). It shares the legacy
field conversion code, including wide coordinates and categorical/string decoding.

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
* Conversion: the row-group DataFrame and owned row vector temporarily overlap.
  An exhausted vector allocation can overlap the next group conversion until it
  is replaced. Strings are
  copied as required by the existing row API. No columnar public API is added.
* Output: up to batch_rows owned rows, except an oversized complete read. C ABI
  conversion creates temporary descriptors and C strings; Python creates record
  objects while those native buffers still exist.
* Read staging: rows+matching_alignments needs no full-read staging. Other
  combinations currently accumulate a raw complete read before deciding/filtering;
  ready records plus one lookahead row can overlap the current batch and decoder.
  Thus even a fully rejected very large read can require substantial temporary
  memory. Complete-read filtering with rows still buffers a complete read.
* Metadata: shard path list, current file footer/row-group metadata and a temporary
  metadata clone. These scale with shard count / row-group count, not decoded
  shard rows. Metadata cloning costs grow with the number of row groups.

**batch_rows is output row control, not a strict byte or RSS memory limit.**
A shard consisting of one huge row group still requires that decoding unit;
a huge read requires read-sized staging/output where applicable. Changing an
existing file's row-group layout is outside this change. No async prefetch,
parallel reader, remote storage, format change or new dependency is introduced.

See [measurement and validation results](streaming-results.md) for reproducible
conditions, limitations, commands and observed memory/throughput.
