# Changelog

## 0.2.3 — 2026-10-09

- Simplify installation, CLI, Python, Rust and concat workflow guides; shorten
  the main getting-started pages and remove repeated setup instructions.
- Highlight selective reads, multi-fragment preservation, batch-based I/O and
  measured performance on the homepage, with links to benchmark methods.
- Keep Rust API in top-level navigation and collect detailed operations in a
  reference index; move advanced Rust examples into a separate reference page.
- Provide a complete copyable Rust example and verify the simplified Python
  examples and internal documentation links. Storage schemas and ABI v1 remain
  unchanged.

## 0.2.2 — 2026-10-08

- Add a runnable concat workflow and integration tests: select complete reads
  by region/quality/logical ID, preserve stored fragments and optionally expand
  pairs with independent quality/order thresholds. No format or ABI changes.
- Clarify native CLI command descriptions and use direct `pqsio` commands in
  user documentation, with PATH setup and Bioconda installation instructions.
- Use the polling watcher for documentation build/preview to avoid inotify
  quota exhaustion and empty-site 404 responses without administrator access.
- Define source-release compatibility and separate pairs/concat CPhasing tests.
  CPhasing with PyArrow 10.0.1 remains unsupported for generated concat PQS;
  no minimum compatible PyArrow version is claimed.
- Record validation for the Linux x86-64 source release in the development guide.

## 0.2.1 — 2026-09-20

- Add compression selection to synchronous and parallel writer APIs in Rust,
  Python, C and C++: uncompressed, Zstd, gzip, Brotli, Snappy and LZ4, with
  validated levels where supported. Keep default Zstd, ABI v1 and PQS schemas.
- Replace the Python rich-click CLI with a native Rust executable using clap,
  direct Rust API calls, terminal tables and stderr progress. Preserve the
  existing commands, region syntax, index behavior and Python/C APIs.
- Remove the Python CLI runtime dependencies and pip console-script entry.
  `python -m pqsio` remains an optional launcher for the native executable.

- Migrate documentation to Zensical with concise installation, Python API and
  CLI guides, plus a PQS format reference.
- Add reproducible read/write benchmarks comparing text with uncompressed PQS
  and gzip with default PQS, with measured figures and downloadable summaries.
- Document a known external-reader limitation: CPhasing with PyArrow 10.0.1
  rejects concat Parquet footers in compatibility tests; native pqsio reads pass.

## 0.2.0 — 2026-09-19

- Add a rich-click CLI with direct BAM/PAF/concat conversion commands and native
  BAM/PAF decoding, grouping, compressed input and output.
- Add Rust pairs-text/PQS-to-Cooler conversion, human-readable bin sizes,
  projected PQS reads, bounded sorting and parallel pixel compression with
  single-threaded HDF5 writing.
- Add info, head/view, pairs/concat/TSV export, quality statistics and stage
  progress, with corresponding Python and additive C APIs.
- Add region query and q0/q1 index management commands. CLI regions use
  CHROM:START-END; eligible queries automatically build missing indexes,
  with explicit opt-out and rebuild controls.
- Expand API/CLI documentation, regression tests and reproducible performance
  scripts. Retain ABI v1 and the existing pairs/concat storage format versions.

## 0.1.0 — 2026-09-10

- Prepare the standalone Rust library, C/C++ headers and Python bindings for a
  coordinated versioned source release.
- Reorganize language and storage documentation and exclude local environments,
  generated libraries, caches and build outputs from version control.
- Retain Cargo.lock for reproducible builds. The library version is independent
  of the pairs PQS 0.1.0 and concat PQS 0.2.0 storage format versions.

## 0.0.14 — 2026-09-09

- Add bounded concat-to-pairs conversion for Rust, Python, and C/C++, with
  MAPQ/order filters, deterministic IDs, midpoint coordinates, and `cn.info`
  propagation.
- Expand directly between column batches and reuse conversion/encoding workers
  and buffers for bounded, source-ordered parallel output.
- Add conversion acceptance coverage, API documentation, and reproducible
  benchmark scripts without adding runtime dependencies.

## 0.0.13 — 2026-09-09

- Accelerate native numeric frame-to-column conversion with typed, chunk-aware
  traversal while retaining conversion and null-error behavior.
- Avoid a duplicate complete-read column copy when streaming concat data has no
  region or MAPQ predicate, and add regression coverage for the filtered path.
- Use fixed ABI layouts when decoding `Pair` and `Alignment` rows in Python,
  with boundary coverage and reproducible synthetic benchmark reports.

## 0.0.12 — 2026-09-09

- Add optional structured `cn.info` read, set, and update APIs for Rust, Python,
  and C, including staged writer configuration.
- Validate explicit copy-number declarations against known contigs, distinguish a
  missing file from a legal empty file, and surface diagnostics through inspect
  and quick validation.
- Propagate explicit copy-number declarations through merge and subset with
  conflict checks, bounded provenance reporting, and atomic replacement safety.

## 0.0.11 — 2026-09-09

- Add q0-only, order-preserving subset export for Rust, Python and the C ABI,
  with selection by MAPQ, contig, region and public read ID.
- Support pairs endpoint policies and concat matching-alignment or complete-read
  modes, preserve the contig table/logical IDs and regenerate output q1/counts.
- Use staged publication and optional bounded provenance sidecars; reject
  invalid source semantics, paths, options and records while cleaning failed
  staging outputs.

## 0.0.10 — 2026-09-09

- Add optional q0/q1 row-group summary indexes and exact region queries for
  Rust, Python and the C ABI, with read-only source access and index lifecycle
  isolation by quality partition.
- Support automatic q1 selection for positive-MAPQ pairs/global concat matching
  reads, while complete-read and shard-local concat queries retain q0 semantics.
- Provide `auto`, sequential `off` and indexed `require` modes with integrity
  checks, explicit fallback diagnostics, cursor statistics and documented
  synthetic measurements.

## 0.0.9 — 2026-09-09

- Add caller-ordered, q0-only streaming merge APIs for Rust, Python and the C
  ABI; union contigs, remap chromosome IDs, preserve pairs IDs and assign fresh
  global concat read IDs.
- Regenerate q1 from q0, use staged atomic publication, and optionally write
  JSONL source/read provenance without copying application sidecars.
- Reject duplicate/overlapping paths, incompatible formats or contigs, invalid
  read grouping and unsupported input semantics; add rollback and cross-binding
  regression coverage.

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
