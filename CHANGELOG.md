# Changelog

## 0.0.8 — 2026-09-09

- Add non-executing structured metadata parsing shared by readers.
- Add read-only Rust/Python inspect and quick/full validate APIs, additive C
  JSON callbacks, bounded issue examples and raw row-group checks.
- Distinguish proven errors, unsupported/incomplete checks and ordered q0/q1
  mismatches; document compatibility and memory limits.
## 0.0.7 — 2026-09-09

- Add owned column-batch output to `StreamingReader` in Rust, C, C++17 and
  Python, with the same cursor, batching, grouping and filtering semantics as
  streaming row output.
- Decode stream row groups into shared typed column buffers; materialize row
  objects only for the row API, while preserving cross-row-group read offsets
  and buffer lifetime guarantees.
- Keep all existing ABI entry points, disk schemas and legacy readers intact;
  add capability detection and cross-language tests for column output,
  interleaved row/column reads, terminal errors and ownership.

## 0.0.6 — 2026-09-09

- Add `StreamingReader` APIs for Rust, C, C++17 and Python. They form row
  batches across Parquet row groups and shards independently of on-disk shard
  boundaries.
- Support bounded row batches or complete-read batches, together with explicit
  concat matching-alignment and complete-read MAPQ filtering policies.
- Keep ABI v1, disk formats and legacy reader behavior unchanged; provide
  old-native-library capability detection, cross-language regression tests and
  documented memory/throughput measurements.

## 0.0.5 — 2026-09-09

- Add a standalone, lockfile-pinned Pixi development environment for the
  supported Linux x86-64 and aarch64 platforms, including Rust, C/C++ and
  Python test tooling.
- Provide focused build, test, lint and opt-in benchmark tasks; keep release
  artifacts an explicit separate task.
- Document the self-contained workflow and compatibility-test boundary, and
  honor activated C and C++ compiler settings in native ABI tests.

## 0.0.4 — 2026-09-09

- Add synchronous typed columnar pairs/concat writing and reading in Rust, C,
  C++17 and Python, without intermediate row-object batches.
- Validate complete submissions before acceptance; preserve complete reads,
  cross-call read ID ordering, MAPQ filtering and legacy shard-local IDs.
- Add owned native read batches, C++ RAII and safe Python array copies with
  explicit buffer lifetimes and old-library capability detection.
- Share storage/schema logic with row APIs; retain ABI v1, Polars 0.49.1,
  existing disk formats and a standard-library-only Python package.
- Add cross-language regression tests, buffer contracts and reproducible
  row/column benchmarks documenting measured performance and copy costs.

## 0.0.3 — 2026-09-09

- Add ordered multi-producer writing with bounded input batches and a fixed
  shard encoding pool, available in Rust, C, C++17 and Python.
- Propagate background failures, wake blocked submitters, join workers before
  cleanup, and publish only after successful completion.
- Add concurrency/lifecycle tests and synchronous versus 1/2/4/8-worker benchmarks.
- Preserve ABI v1, Polars 0.49.1 and existing PQS schemas.

## 0.0.2 — 2026-09-09

- Reuse typed integer buffers and decode local categorical dictionaries once per
  shard instead of allocating temporary strings and looking up columns per row.
- Add owned Rust write methods; avoid duplicate string copies in C/Python writes
  and reuse owned strings for C reader callbacks.
- Count complete concat reads while accepting them, without per-shard HashSets.
- Add validated multi-read concat submissions to Rust, C, C++ and Python.
- Optimize fixed-layout Python record encoding while retaining validation.
- Preserve ABI v1, PQS schemas, q0/q1 behavior and original public methods.
- Add boundary/compatibility tests and reproducible v0.0.1 acceptance benchmarks.

## 0.0.1

Initial pairs/concat PQS readers and writers for Rust, Python, C and C++.
