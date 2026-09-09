# Region query validation and synthetic results

## Independent q1 indexes

Current APIs build and query independent q0/q1 row-group indexes. Build q1 with
`build_index(path, quality="q1")`. `index="require"` targets the selected source
and no longer switches positive-MAPQ matching queries back to q0. The original
q0 index location, schema, Rust builder and C ABI are preserved. See
[API/schema/memory contracts](query.md).

Validation commands, run from `pqsio/` in the existing Pixi environment:

```sh
pixi run build
pixi run cargo test --locked --profile dev-release -j 4
pixi run python -m unittest discover -s tests -p test_query.py -v -f
pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings
pixi run python scripts/query_bench.py > docs/query-q1-benchmark.json
```

The legacy regression run used the following command:

```sh
pixi run python - <<'PY'
import sys, unittest
sys.path.insert(0, 'tests')
names = ['test_python', 'test_columns', 'test_native', 'test_streaming',
         'test_merge', 'test_inspection']
result = unittest.TextTestRunner(verbosity=1).run(
    unittest.defaultTestLoader.loadTestsFromNames(names))
raise SystemExit(not result.wasSuccessful())
PY
```

Results: build passed; 29 Rust tests (including physical empty q1 groups and
q1 source mutation during build) passed; 23 Python query tests passed;
54 existing Python/C/C++ tests passed; strict Clippy passed. Query tests cover
q1 skipping for pairs and concat; exact all-field/order/duplicate/ID agreement
against independent q0 and sequential q1 oracles; separate invalidation and
rebuilds; manifest quality mismatch; missing/stale/corrupt/unsupported q1 indexes;
old-native capability checks; independent build locks; read-only source files;
late index failure without restart; and explicit q1 construction for shard-local
concat even with unreadable q0. Complete-read and shard-local queries continue
using sequential q0. No runtime dependencies or versions changed.

### q1 measurement

The original q0 fixture has 32,768 pairs; its q1 view has 24,576 pairs in 24
groups of 1,024 rows. Query: chr1:[50000,51000), MAPQ >=30, either, batch_rows=4096.
Five fresh worker processes per mode, alternating order, with parent reads
warming the OS page cache. Zero summary cache. Timings include query open,
identity/index preflight, filtering, column transfer, hashing, stats and close;
fixture construction and index building are excluded and separately recorded.
Linux VmHWM measures each worker process peak, including startup. No cold-cache
or physical-read-byte claim is made.

| Distribution / mode | Candidate / total | Decoded rows | Returned rows | Median ms | Median peak RSS MiB | Maximum peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| clustered / off | 24/24 | 24,576 | 768 | 8.669 | 31.457 | 31.598 |
| clustered / require | 2/24 | 2,048 | 768 | 3.489 | 31.504 | 31.520 |
| mixed / off | 24/24 | 24,576 | 757 | 8.483 | 31.438 | 31.582 |
| mixed / require | 24/24 | 24,576 | 757 | 8.737 | 31.418 | 31.500 |

| Distribution | Fixture seconds | q1 index build seconds | Manifest bytes | Source inventory serialized bytes |
|---|---:|---:|---:|---:|
| clustered | 0.2903 | 0.2578 | 575 | 305 |
| mixed | 0.2510 | 0.2636 | 576 | 306 |

Every indexed/sequential trial within each distribution has the same complete
column-buffer SHA-256 digest. The clustered q1 index skips 22/24 (91.667%) of
groups; the mixed index skips none and adds a small overhead. RSS differences
are small. These results do not establish genome-scale, cold-cache or general
performance benefits. All trials: [query-q1-benchmark.json](query-q1-benchmark.json).

Files changed for q1 indexing: `src/query.rs`, `src/lib.rs`, `src/ffi.rs`,
`include/pqsio.h`, `python/pqsio/query.py`, `tests/test_query.py`,
`scripts/query_bench.py`, `README.md`, `docs/query.md`, this report and
`docs/query-q1-benchmark.json`. No additional changes to the public StreamingReader
policy were needed. The historical q0 measurements below are retained, not
relabelled as q1 results. The current benchmark driver compares sequential and
indexed **q1**, so its results need not match the old q0 table.

Limits remain: q1 completeness/order is trusted rather than validated against
q0 on each query; concurrent mutation is not snapshot-isolated; checksums are
non-cryptographic; a large row group or complete read can exceed output batch
limits. No large-data stress, cold-cache or cross-platform benchmark was run.

## Earlier sequential-q1 implementation (historical)

This historical implementation selected q0 for require; that rule is superseded
by the independent q1-index implementation above. The source-switching update passed the following checks in the same Pixi
`dev-release` environment:

```sh
pixi run build
pixi run cargo test --locked --profile dev-release -j 4
pixi run python -m unittest discover -s tests -p test_query.py -v -f
pixi run python -m unittest discover -s tests -p test_streaming.py -v
pixi run python -m unittest discover -s tests -p test_merge.py -v
pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings
```

Results: native build passed; 28 Rust tests, 18 query tests, 11 streaming tests,
and 6 merge tests passed; strict Clippy passed. The query fixtures now preserve
q1 when their q0 data are rewritten. The independent oracle still reads q0
through the old Reader and applies the exact conditions separately. Tests cover
MAPQ 0/1/30/61, pairs and concat, row and complete-read batch boundaries,
per-partition counters, all index modes, duplicate rows, and stable IDs.
Additional cases corrupt q0/the index while q1 is selected, corrupt/remove q1,
and remove q1 entirely for complete-read or shard-local queries. They verify
that only the chosen partition is read and that source errors propagate.

Files updated for switching: `src/query.rs`, `src/streaming.rs`,
`python/pqsio/query.py`, `tests/test_query.py`, `include/pqsio.h` (comments),
`README.md`, `docs/query.md`, and this report. No ABI signatures, runtime
dependencies, source datasets or public StreamingReader data-selection policy
changed. No new latency or memory benchmark was run. q1 completeness/order is
trusted under the existing PQS contract rather than verified by rescanning q0.

## Original q0-only measurement and broader regression

Measured on 2026-09-09 in the repository Pixi environment: Rust 1.91.1,
Polars Rust 0.49.1, Python 3.11, test-only Python Polars 0.20.29.
All native builds use `dev-release`; no dependency/lockfile changes.

## Reproduction

Run from the `pqsio/` directory:

```sh
pixi run cargo build --locked --profile dev-release -j 4
pixi run cargo test --locked --profile dev-release -j 4
pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings
pixi run python -m unittest discover -s tests -p test_query.py -v
pixi run python -m unittest discover -s tests -p 'test_*.py' -v
pixi run python scripts/query_bench.py > docs/query-benchmark.json
```

## Test outcomes

- Native build and strict Clippy: passed.
- Rust: all 28 tests passed (9 unit, 19 integration), including 5 new index/query unit tests.
- New Python query suite: all 15 test methods passed (8.096 s), including exhaustive small combinations, fixed-seed pairs/concat cases and the u64 maximum. The test file is `tests/test_query.py`.
- Broad Python regression before the final additional u64-maximum test: 77 test methods, 75 passed, 2 external CPhasing Python import errors. All then-existing 14 query tests passed. Existing C/C++ header/ABI, real archived v0.0.1 library, streaming, columns, metadata inspection/validation, merge and parallel-writer tests passed.
- The two errors were `test_existing_cphasing_python_reader` and `test_parallel_output_with_existing_cphasing_reader`: standalone Pixi cannot import `cphasing`. Retrying just those tests after adding `../CPhasing` to Python imports reached the local package but failed on missing `click`. No packages were installed. These external Python interoperability checks remain unvalidated; they are not reported as passing or silently skipped.

New coverage includes either/both with MAPQ, independent endpoint false positives,
overlapping region unions and true duplicate source rows, multi-contig groups,
pairs position conversion and adjacent intervals, coordinates above 2^32 and
at the u64 maximum, all/no matches, empty datasets/shards, concat reads crossing
groups/shards, complete-read sequential fallback, legacy shard-local ID mapping,
row and complete-read batch policies, missing/stale/corrupt/unsupported indexes,
invalid coordinates/contigs/null quality, rebuild failure preserving CURRENT,
injected mid-build source change, read-only source hashes/mtimes, early stop,
terminal late corruption, descriptor release and actual old-library detection.

Both zero-group empty shards and physical zero-row groups are tested. The Rust
fixture uses the existing batched Parquet writer to emit groups with row counts
`[0, 1, 0]`, verifies those footer counts, builds an index and compares both
query paths: only the middle group decodes and both empty groups are skipped.
A separate wire-level test also checks a zero-row index header. No external
dataset, whole-genome or large-output workflow was run.

## Measurement method

Each distribution contains 32,768 synthetic pairs in one q0 shard with 32
row groups of 1,024 rows. Footer statistics are disabled. Query is
`chr1:[50000,51000)`, MAPQ >=30, `either`, batch_rows=4096. In the clustered
fixture each group occupies one narrow coordinate range; the mixed fixture
randomly spreads those ranges through every group. Each contains chr1/chr2.
Five fresh worker processes per mode are run in alternating mode order.

Wall time covers query construction, source identity/footers, full index
preflight where applicable, decoding, exact filtering, Python column transfer,
output hashing, statistics and close. Fixture creation and index building are
**excluded** from query times and separately reported below. The parent reads
source and index files before each trial to warm the OS page cache. There is
no strict cold-cache result, privileged cache flushing or process-affinity
control. All summary caches are disabled, including in warmed trials.

Peak RSS is Linux `/proc/self/status` **VmHWM** for the worker address space
since exec, including Python/native startup. An initial diagnostic using
`getrusage().ru_maxrss` showed an inherited parent high-water floor and was
replaced; the final raw artifact uses VmHWM. These are process peaks, not a
strict library heap limit or per-query allocation delta.

| Distribution / mode | Candidate / total | Decoded groups | Decoded rows | Returned rows | Median ms | Median peak RSS MiB | Maximum peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| clustered / off | 32/32 | 32 | 32,768 | 768 | 10.494 | 31.699 | 31.820 |
| clustered / require | 1/32 | 1 | 1,024 | 768 | 3.301 | 30.836 | 31.012 |
| mixed / off | 32/32 | 32 | 32,768 | 757 | 10.571 | 31.457 | 31.688 |
| mixed / require | 32/32 | 32 | 32,768 | 757 | 10.732 | 31.570 | 31.637 |

| Distribution | Fixture creation seconds | Index build seconds | Manifest bytes | Serialized source inventory bytes |
|---|---:|---:|---:|---:|
| clustered | 0.2522 | 0.2436 | 572 | 303 |
| mixed | 0.2153 | 0.2658 | 574 | 304 |

Every trial produced the same complete column-buffer SHA-256 digest for
indexed and sequential output within its distribution, including order, IDs
and all fields. The concentrated fixture skips 31/32 = **96.875%** of groups;
the mixed fixture skips **0%** and incurs a small index overhead. RSS differences
are small and do not establish a meaningful memory improvement.

Actual decoded groups/rows establish a reduction in data decoding for this
clustered fixture. They do not measure physical disk bytes: source footers and
all summary partitions are still read for preflight, and the OS cache matters.
These small synthetic results must not be extrapolated to whole genomes, other
shard/group layouts, storage devices, cold caches or concurrent modification.
No concat performance, power-loss recovery, cross-platform behavior, or very
large row-group/complete-read memory stress measurement is claimed.

All trials and exact counters: [query-benchmark.json](query-benchmark.json).
Schema, semantics, failure policy and memory controls: [query.md](query.md).

## Changed files

- Rust: `src/query.rs` (new public query/index API and failure tests), `src/streaming.rs` (shared exact predicate, pruning hook, counters), `src/lib.rs` (exports), `src/ffi.rs` (additive bridge).
- C: `include/pqsio.h` (new region struct and functions; existing ABI retained).
- Python: `python/pqsio/query.py`, `python/pqsio/__init__.py`.
- Verification: `tests/test_query.py`, `scripts/query_bench.py`.
- Documentation/results: `README.md`, `docs/query.md`, `docs/query-results.md`, `docs/query-benchmark.json`.

No lockfile, runtime dependency, source PQS data or unrelated project files were
changed. Released as v0.0.10.
