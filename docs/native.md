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
Other status functions return 0 or -1, except `reader_kind` (0 pairs/1 concat).
Read `pqsio_last_error()` immediately after failure: the error is thread-local
and the next API call may replace it. Rust panics are caught at the ABI boundary.
A read error or failed callback may consume the current shard; abort/reopen the
reader after an error. Explicitly finish writers; destruction only cleans up.
