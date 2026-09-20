# C and C++ API

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
Most other status functions return 0 or -1, except `reader_kind` (0 pairs/1 concat)
and the conversion extensions described below.
Read `pqsio_last_error()` immediately after failure: the error is thread-local
and the next API call may replace it. Rust panics are caught at the ABI boundary.
A read error or failed callback may consume the current shard; abort/reopen the
reader after an error. Explicitly finish writers; destruction only cleans up.

## Compression

The additive C entry points `pqsio_writer_open_with_compression` and
`pqsio_parallel_open_with_compression` take a codec string and an optional
`const int32_t *level`, just before the output-handle argument. Pass `NULL`
for the codec default level. A pointer to zero requests level 0 explicitly.
Invalid settings return -1 and leave the output handle NULL.

```c
int32_t level = 6;
pqsio_writer *writer = NULL;
/* Check the return value, then write, finish and destroy as usual. */
int32_t status = pqsio_writer_open_with_compression(
    "compressed.pairs.pqs", 0, contigs, n_contigs, 100000,
    "zstd", &level, &writer);
```

C++17 constructors accept `pqsio::Compression` after the contig vector:

```cpp
pqsio::Writer writer("compressed.pairs.pqs", pqsio::Kind::Pairs,
                     {{"chr1", 1000}}, pqsio::Compression{"zstd", 6});
pqsio::ParallelWriter parallel("uncompressed.pairs.pqs", pqsio::Kind::Pairs,
                               {{"chr1", 1000}}, pqsio::Compression{"uncompressed"});
// Write records and call finish() on each writer.
```

Existing constructors and ABI v1 remain unchanged and use default Zstd.
Selecting compression requires the new shared-library symbols. Both quality
partitions use the setting; see [codecs and levels](pqs-format.md#compression).

## Conversion and JSON reports

The declarations in `include/pqsio.h` are authoritative. C++17 callers may use
the same functions directly; `pqsio.hpp` has no dedicated conversion wrapper.

| Function | Supported conversion |
| --- | --- |
| `pqsio_convert_json` | `concat2pairs`, one worker |
| `pqsio_convert_parallel_json` | `concat2pairs`, explicit worker count |
| `pqsio_import_json` | `bam2pairs`, `bam2concat`, `paf2pairs`, `paf2concat` |
| `pqsio_pairs2cool_json` | pairs PQS/text to Cooler |

```c
#include <stdio.h>
#include "pqsio.h"

static int32_t print_report(const uint8_t *data, size_t size, void *user) {
    (void)user;
    return fwrite(data, 1, size, stdout) == size ? 0 : -1;
}

int main(void) {
    int32_t status = pqsio_pairs2cool_json(
        "sample.pairs.pqs", "sample.10k.cool",
        10000,       /* bin size: integer bp */
        4000000,     /* accepted pairs per sorting run */
        65536,       /* batch rows */
        1, 4,        /* minimum MAPQ, workers per stage */
        NULL, NULL,  /* contigsizes (text only), scratch directory */
        print_report, NULL);
    if (status != 0) {
        fprintf(stderr, "%s\n", pqsio_last_error());
        return 1;
    }
    return 0;
}
```

C has no argument defaults: supply every value. Imports use `SIZE_MAX` for
unlimited `max_order`, explicit `min_order` (2 for pairs, 1 for concat), and
0/1 for `include_secondary` and `five_prime`. Optional `contigsizes`/`tmpdir`
are `NULL`; bin sizes use integer bp rather than unit strings.

`pqsio_import_json` and `pqsio_pairs2cool_json` return 0 on success, -2 for
invalid options/data, and -1 for other failures. Concat conversion returns
0 or -1. JSON callback buffers are borrowed for the callback duration and
are **length-delimited, not necessarily NUL-terminated**. Copy them to retain
them. Return 0, do not reenter pqsio, and do not throw across the boundary.
Callback failure after publication leaves the completed output in place.

`pqsio_inspect_json` and `pqsio_validate_json` use the same callback contract.
Successful report delivery is not proof that a dataset is valid: examine the
validation report's `status`. These are PQS operations, not Cooler validators.
Resolve additive symbols when supporting older shared libraries. See the
[API overview](api.md) for the cross-language mapping and operation contracts.

## Summaries, text export and progress

The additive symbols `pqsio_info_json`, `pqsio_export_json` and
`pqsio_set_progress_callback` support native browsing/export and optional
thread-local stage notifications. Export receives a JSON options object;
null output selects stdout. See [browse and export](browse.md) for option keys,
return codes and callback lifetime contracts. The C++ wrapper does not yet
provide member functions for these operations; use the C entry points.

`pqsio_stats_json(path, min_mapq, callback, user)` scans q0 and returns aggregated
[quality statistics](stats.md). `pqsio_index_status_json(path, quality, callback,
user)` checks a q0 (0) or q1 (1) index. Both return 0 or -1 using the same JSON
callback lifetime contract. For index status, successful delivery can report
`missing` or `invalid`; examine the JSON `status` field. Record data remains
native; there is no new C++ convenience wrapper.
