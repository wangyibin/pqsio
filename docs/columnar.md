# Synchronous columnar extension v1

The columnar extension adds symbols and methods; C ABI version remains **1**.
PQS pairs 0.1.0 / concat 0.2.0 disk formats, Polars 0.49.1, row methods and
C structure layouts/signatures are unchanged. No runtime dependency is added.
Parallel producers retain their existing row API; columnar submission is synchronous.

## Python

```python
from array import array
from pqsio import PairColumns, ConcatColumns, PairsWriter, ConcatWriter, Reader, pack_strings

ids, text = pack_strings(["读段", ""])  # offsets [0, 6, 6]; no terminator
batch = PairColumns(
    read_id_offsets=ids, read_id_bytes=text,
    chrom1=array("I", [0, 0]), pos1=array("Q", [1, 10]),
    chrom2=array("I", [0, 0]), pos2=array("Q", [100, 200]),
    strand1=array("B", [ord("+"), ord("-")]),
    strand2=array("B", [ord("-"), ord("+")]), mapq=array("B", [0, 60]),
)
with PairsWriter("example.pairs.pqs", {"chr1": 1000}, chunk_size=10000) as w:
    w.write_columns(batch)

reasons, text = pack_strings(["pass", ""])
alignments = ConcatColumns(
    read_offsets=array("Q", [0, 2]),  # one complete two-alignment read
    read_idx=array("Q", [1, 1]), read_length=array("I", [200, 200]),
    read_start=array("I", [0, 100]), read_end=array("I", [100, 200]),
    strand=array("B", [43, 45]), chrom=array("I", [0, 0]),
    start=array("Q", [0, 500]), end=array("Q", [100, 600]),
    mapping_quality=array("B", [0, 60]), identity=array("f", [0.5, 1.0]),
    filter_reason_offsets=reasons, filter_reason_bytes=text,
)
with ConcatWriter("example.concat.pqs", {"chr1": 1000}) as w:
    w.write_columns(alignments)

with Reader("example.concat.pqs", min_mapq=1) as r:
    for batch in r.iter_columns():
        positions = memoryview(batch.start)
        print(list(batch.read_offsets), list(positions))  # [0, 1], [500]
# `positions` keeps its Python-owned array alive after reader/batch destruction.
```

`PairColumns` and `ConcatColumns` are containers of buffers, not lists of row
objects. `len(batch)` is the alignment/pair count. Construction stores supplied
buffers without copying; submission validates types and all business fields.
`pack_strings()` accepts an iterable of strings, encoding each once into one
packed byte array. Users can provide already packed buffers instead.

Inputs accept `array` or one-dimensional C-contiguous `memoryview` (also bytes
for byte fields). Formats must match the table, native endian, natural alignment;
strided, differently typed or wrong-width buffers are rejected. Writable buffers
are borrowed via ctypes until the synchronous call returns; readonly buffers
are copied once into temporary ctypes storage. Python owners and buffer exports
are kept alive for that call. **Do not mutate or resize inputs from another
thread while submitting**; the GIL is not an input synchronization mechanism.

Returned buffers are mutable Python `array`s. Each column is bulk-copied once
from the native batch, directly into the final array (zero initialization also
writes this destination). The native batch is destroyed before yielding.
Subsequent reads and closing the reader cannot invalidate arrays or their
memoryviews. There is no public borrowed native Python view or implicit row
conversion. `iter_columns()` and `iter_batches()` advance the same reader cursor;
interleaving consumes subsequent shards, it does not reread them.

## Exact buffer contract

All numeric buffers are contiguous native-endian primitives, with **N elements**
unless noted. No nullable values or validity bitmap. C `len` is `size_t` and
counts elements, not bytes. Rust borrowed views are `&[T]`, owned columns `Vec<T>`.

| Format | Field(s) | Rust / C | Python format | Meaning |
|---|---|---|---|---|
| pairs | `chrom1`, `chrom2` | u32 / uint32_t | I (4 bytes) | Index into reader/writer ordered contigs |
| pairs | `pos1`, `pos2` | u64 / uint64_t | Q (8 bytes) | 1-based; positive and ≤ contig length |
| pairs | `strand1`, `strand2`, `mapq` | u8 / uint8_t | B | ASCII + / -; MAPQ 0–255 |
| pairs | `read_id_offsets` | u64 / uint64_t | Q | N+1 string byte offsets |
| pairs | `read_id_bytes` | u8 / uint8_t | B | Packed UTF-8, byte length B |
| concat | `read_offsets` | u64 / uint64_t | Q | R+1 alignment offsets, R read groups |
| concat | `read_idx` | u64 / uint64_t | Q | Global read ID, repeated on each alignment |
| concat | `read_length`, `read_start`, `read_end` | u32 / uint32_t | I | Query length and 0-based half-open interval |
| concat | `chrom` | u32 / uint32_t | I | Contig ID |
| concat | `start`, `end` | u64 / uint64_t | Q | 0-based half-open reference interval |
| concat | `strand`, `mapping_quality` | u8 / uint8_t | B | ASCII + / -; MAPQ 0–255 |
| concat | `identity` | f32 / float | f (4 bytes) | Finite; no new [0,1] restriction |
| concat | `filter_reason_offsets` | u64 / uint64_t | Q | N+1 string byte offsets |
| concat | `filter_reason_bytes` | u8 / uint8_t | B | Packed UTF-8, byte length B |

String offsets start at 0, are nondecreasing and end exactly at B. Every adjacent
slice must independently decode as UTF-8 (offsets cannot split a codepoint).
Equal offsets encode empty strings. No NUL terminator is needed; embedded NUL
is supported by columnar storage/reads, but cannot be returned by the legacy
NUL-terminated C string API (which reports an error). Use NUL-free strings for
cross-path C/Python row compatibility.

Submission read offsets start at 0, strictly increase and end at N. Each interval must have
the same read ID and read length. IDs strictly increase between complete reads
and across calls, including mixed row/column calls. Query intervals satisfy
`0 <= read_start < read_end <= read_length`; reference intervals satisfy
`0 <= start < end <= contig length`. A single complete read can exceed the shard
target and is emitted as one oversized shard.

Empty submission: N=R=B=0, numeric/byte buffers empty, **each string/read offsets
buffer is [0]**. C permits NULL only for zero-length spans; the offsets pointer
therefore cannot be NULL even in an empty batch. Zero-length pointers are not
dereferenced. Empty submissions accept no records and do not flush pending data.
An empty dataset yields EOF immediately. A filtered shard with no surviving
records yields an empty batch with offsets [0], distinct from EOF.

All spans must be properly aligned for their element type; C entry points check
nonempty buffer alignment, NULL/length combinations, size overflow, column
lengths, UTF-8, offsets and field semantics. Structs/handles/output pointers must
also be naturally aligned. **The caller guarantees actual allocation extent,
initialized elements, pointer liveness and exclusive handle access. Rust cannot
validate dangling pointers or invented lengths.** No concurrent mutation or
reentrant use. Input buffers are read-only to the library and borrowed only for
the call; the writer owns copies after return.

## Rust

```rust
use pqsio::{PairColumns, ColumnBatch, Reader, Writer};
fn write(w: &mut Writer, b: &PairColumns) -> anyhow::Result<()> {
    w.write_pairs_columns(b.as_view())?;
    Ok(())
}
fn read(r: &mut Reader) -> anyhow::Result<()> {
    while let Some(batch) = r.next_columns()? {
        match batch {
            ColumnBatch::Pairs(b) => println!("{}", b.pos1.len()),
            ColumnBatch::Concat(b) => println!("{:?}", b.read_offsets),
        }
    }
    Ok(())
}
```

`ConcatColumns::as_view()` supplies `write_concat_columns`. Callers can construct
`PairColumnsView`/`ConcatColumnsView` directly from slices, without allocating an
owned input batch. `Default` produces a valid empty owned batch. Read batches own
all vectors and can outlive the reader; normal Rust borrowing protects views.

## C and C++17

Each C field is a typed `pqsio_span_u8/u32/u64/f32 { const T *data; size_t len; }`.
`pqsio_pairs_columns` and `pqsio_concat_columns` contain the table's fields in
header order; use named initialization or initialize every field explicitly.

```c
/* v is a fully initialized pqsio_pairs_columns of caller-owned buffers. */
int status = pqsio_write_pairs_columns(writer, &v); /* 0 or -1 */
pqsio_column_batch *batch = NULL;
status = pqsio_reader_next_columns(reader, &batch); /* 1 / 0 EOF / -1 */
if (status == 1) {
    pqsio_pairs_columns view;
    if (pqsio_column_batch_pairs(batch, &view) == 0) {
        /* view.pos1.data remains valid even after reader_destroy. */
    }
    pqsio_column_batch_destroy(batch);
}
```

`pqsio_column_batch_concat` is the concat getter. Getting the wrong kind fails.
Getters do not transfer ownership; all returned buffers are immutable until
`pqsio_column_batch_destroy`. Destroy exactly once, after every borrowed view's
last use; destroy(NULL) succeeds. Failure/EOF from next sets the output handle to
NULL. Do not overwrite a live owned handle. Functions catch Rust panics and use
the unchanged thread-local `pqsio_last_error` convention. A read error may consume
a shard; abort/reopen after failure, as with the old reader.

```cpp
pqsio::Writer w(path, pqsio::Kind::Pairs, contigs);
w.write_pairs_columns(columns); // borrowed input descriptor
w.finish();
pqsio::Reader r(path);
while (auto batch = r.next_columns()) {
    auto view = batch.pairs(); // or batch.concat()
    // use view here; batch is move-only and frees its native handle via RAII
}
```

C++ views cannot outlive their owning `ColumnBatch`; moving the owner preserves
buffer addresses, move assignment destroys the previously owned batch. Existing
Writer/Reader RAII remains unchanged. C++17, no C++20 span requirement. Complete
compilable examples are in `tests/columns.c` and `tests/columns.cpp`.

## Capability detection and compatibility

`pqsio_columnar_version()` returns **1**. This is a separate extension version,
not a change to `pqsio_abi_version()`. Dynamically loading C clients must resolve
this symbol before resolving column functions (absent = unsupported). Statically
or normally linked new C/C++ consumers require a library exporting the new
symbols; the operating system linker does not provide automatic old-library
fallback. Existing binaries still use unchanged symbols/layouts.

The Python loader does not require new symbols for old operations. Column methods
probe extension availability before calling it and raise an actionable
`RuntimeError` with old libraries; they never fall back to row conversion. The
current package was tested against the archived actual v0.0.1 shared library.

q0 remains all records; q1 is MAPQ ≥ 1. Higher reader thresholds further filter.
Legacy shard-local concat IDs are mapped deterministically from q0 **before**
filtering using the same function as the row reader. Read offsets describe consecutive groups within the returned shard (historical
global-ID files that split a read across shards are not coalesced). Filtered offsets group
only retained alignments; they do not represent the original complete read.
Do not resubmit filtered batches when the application requires original reads.

All validation runs before any portion of a submitted batch is accepted.
Validation errors leave prior buffered data and last read ID intact. Storage
errors poison the writer; close/drop removes staging, and only successful finish
publishes. Row and column paths share scalar business validation, schema helpers,
Parquet storage/filtering/counting and reader frame loading. Switching between
nonempty row and column submissions flushes pending data and may change shard
boundaries; no complete read is split, output order and logical records persist.
Existing row-only shard packing is unchanged.

## Copies, allocations and limits

- No column path constructs `Vec<Pair>`, `Vec<Alignment>` or Python row lists.
  Numeric extraction may iterate scalars, but fills typed vectors directly.
- Write borrows caller buffers, validates all fields (including allocating a
  temporary vector of string slices), then copies ranges into owned column Vecs.
  Numeric buffers become Polars Series, with further allocation/copy governed by
  Polars. Strings and chromosome/strand IDs are converted to the unchanged string
  / categorical disk schema. q1 filtering and Parquet encoding allocate buffers.
- A shard uses bounded column buffers except oversized reads. The caller's input
  batch and validation string-slice vector scale with submission size. DataFrame
  construction, category conversion, q1 and compression overlap in memory; the
  shard target is not an RSS limit. Column buffers are released at flush rather
  than retained for reuse in this first implementation.
- Read materializes a Parquet DataFrame, optionally remaps IDs/filters, decodes
  categories and copies fields into owned continuous Vecs. Python then copies
  each Vec once to an array; native/Python buffers overlap during this transfer.
  C/C++ getters borrow these Vecs without an additional copy.
- There is no end-to-end zero-copy claim, Arrow/NumPy integration, projection,
  asynchronous column writing, new CLI, cloud storage, or format upgrade.

Run the reproducible benchmark with `pixi run bench-columns`. Small warm-cache results do not predict genome-scale
or cold-storage performance.
