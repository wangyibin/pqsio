# Changelog

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
