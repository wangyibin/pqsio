# Public StreamingReader q1 selection validation

Validated on 2026-09-09 with the project Pixi environment and dev-release profile.
Public Rust streaming selection now uses the existing Reader partition policy;
C, C++ and Python share that implementation without an ABI signature change.

Commands and results (run from pqsio):

- `pixi run build`: passed (locked dev-release build).
- `pixi run cargo test --locked --profile dev-release -j 4`: 29 passed.
- `pixi run python -m unittest discover -s tests -p test_streaming.py -v -f`: 14 passed, including compiled C/C++ checks.
- `pixi run python -m unittest discover -s tests -p test_query.py -v -f`: 23 passed.
- `pixi run python -m unittest discover -s tests -p test_merge.py -v -f`: 6 passed.
- `pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings`: passed.
- `git diff --check`: passed.

New tests compare every returned field and ordering against a q0 oracle at
MAPQ 0, 1, 30, 61 and 255, with multiple batch sizes and both concat boundaries.
They cover pairs, global concat, shard-local ID fallback, complete-read q0
selection, missing/corrupt partitions, late errors, early close and descriptors.
Fixtures use different physical q0/q1 row-group layouts while preserving rows.
The benchmark fixture now writes both quality views consistently.

No new performance measurements were run for this streaming change. The results
below are historical q0 streaming measurements, not measurements of q1 selection.
q1 completeness remains an input-format assumption; no new dependency or cache
was introduced, and the existing oversized row-group/read memory limits remain.

---

# Streaming validation and measurements — 0.0.6

Measured on 2026-09-09, Linux x86-64, project Pixi environment, Rust 1.91.1,
Polars Rust 0.49.1, Python 3.11 and fixture-only Python Polars 0.20.29.
Build: locked Cargo dependencies, dev-release, four build jobs. Reader decoding
explicitly uses ParallelStrategy::None (no parallel reading). The activated
Polars/Rayon thread limits are four. Shared host, no CPU pinning or cold-cache
control; these numbers are illustrative, not universal performance guarantees.

## Reproduction

From pqsio:

```sh
pixi run build
pixi run test
pixi run lint
pixi run bench-streaming
```

The independent opt-in script creates one synthetic shard per case, target
row_group_size=4096 and batch_rows=1024, with 20,000 / 80,000 / 320,000 alignments.
Ordinary reads contain four alignments; single_read puts all rows into one read.
All MAPQs are 60, threshold 30. The string fields are short and repetitive.
Two new processes per case stream through Python dataclass batches and discard
them immediately; no list of batches is accumulated. Fixture creation occurs
only in the parent. Peak is the child process's `/proc/self/status` VmHWM,
including native decoding, C ABI conversion and Python objects. Initial
ru_maxrss measurements were discarded because fork/exec inherited a parent's
historical peak; they are not included here. Timing includes opening and closing
the dataset plus reading, but excludes Python import and fixture construction.

Raw run records are written to `tests/output/streaming-measurements.json` and
fixtures are removed at script exit. The table gives maximum peak across the
two repetitions and mean rows/second. No absolute memory threshold is asserted
in unit tests.

| Read layout | Boundary / filter | Shard rows | Peak MiB | Mean rows/s |
|---|---|---:|---:|---:|
| ordinary | rows / matching_alignments | 20,000 | 27.02 | 163,483 |
| ordinary | rows / matching_alignments | 80,000 | 28.31 | 166,478 |
| ordinary | rows / matching_alignments | 320,000 | 28.39 | 172,972 |
| ordinary | complete_reads / complete_reads | 20,000 | 27.20 | 168,888 |
| ordinary | complete_reads / complete_reads | 80,000 | 27.99 | 166,607 |
| ordinary | complete_reads / complete_reads | 320,000 | 28.57 | 173,403 |
| single_read | rows / matching_alignments | 20,000 | 26.98 | 173,473 |
| single_read | rows / matching_alignments | 80,000 | 27.75 | 177,311 |
| single_read | rows / matching_alignments | 320,000 | 28.49 | 174,558 |
| single_read | complete_reads / complete_reads | 20,000 | 36.66 | 166,152 |
| single_read | complete_reads / complete_reads | 80,000 | 67.86 | 167,182 |
| single_read | complete_reads / complete_reads | 320,000 | 193.71 | 166,932 |

Ordinary read peak stays around 27–29 MiB as shard rows increase sixteenfold;
there is no near-linear resident growth with shard row count in these conditions.
Single huge reads stay bounded in rows+matching_alignments, while complete-read
output reaches about 194 MiB for 320,000 alignments. Native read staging and C
callback descriptors/strings overlap with Python record allocation for this
oversized batch. The maximum output batch in that case is exactly 320,000 rows.
These measurements do not claim byte-level bounds for arbitrary row widths,
large dictionaries, metadata footers, or a single enormous row group.

## Validation record

* `pixi run build`: passes, no dependencies or lockfiles changed.
* `pixi run test`: passes legacy Rust, Python, columnar, C/C++, parallel-writing
  suites plus the new streaming suite. Final focused additions are verified with
  `pixi run test-streaming` (9 tests, including compiled C11/C++17 consumers).
* Rust tests: 17 passed (1 unit + 16 storage tests), including direct new Rust API
  combinations, target sizes, oversized reads, order/field equivalence and EOF.
* Existing Python/native suites: 27 passed (9 row, 10 column, 4 native, 4 parallel).
* New streaming suite: 9 passed. Covers internal row groups/global shard read
  continuity; shard-local mapping before filtering; all four boundary/filter
  combinations; target 1, zero, negative and overflow; split and packed shards;
  empty data/shards, full filtering, exact and partial batches; long coordinates;
  poisoned reads/callbacks; explicit close and descriptor release; real archived
  v0.0.1 native-library capability fallback, alongside legacy Reader use.
* `pixi run lint`: passes strict Clippy with warnings denied.
* One initial attempt to pass multiple Pixi task names as positional arguments
  failed at unittest argument parsing; corrected to the aggregate `pixi run test`.
  This was a command invocation error, not a test failure.

## Not validated / limits

No full CPhasing integration suite or genome pipeline, aarch64/Windows build,
remote I/O, cold-cache I/O comparison, strict per-buffer allocator profiling,
or exhaustive arbitrary Parquet codec/layout corpus was run. Performance was
measured for two representative policy combinations; all four are correctness
tested. The named archived old library is available here; that check skips if
it is absent in another checkout. The independent measurement needs Linux
/proc, and is not part of default tests. Shard path/footer metadata grows with
file/row-group counts. Oversized decode units and reads remain explicit limits.
