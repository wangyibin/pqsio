# pqsio

pqsio provides Rust storage for **pairs PQS 0.1.0** and **alignment-level concat
PQS 0.2.0**, with C, C++17 and dependency-free Python bindings. It shares
CPhasing’s storage conventions while remaining independent of its algorithms.

## Get started

1. [Build the native library](installation.md) in the Pixi development environment.
2. Read and write data using [Python](python.md), [Rust](rust.md) or [C/C++](native.md).
3. Check the [coordinate, ID and publication contracts](storage.md).

## Read and write

- [Streaming reads](streaming.md): control batches and complete-read boundaries.
- [Columnar I/O](columnar.md): typed columns, packed IDs and ownership rules.
- [Parallel writing](parallel.md): worker queues, ordering and memory bounds.

## Work with datasets

- [Inspection and validation](inspection.md)
- [Region queries and indexes](query.md)
- [Subset export](subset.md)
- [Dataset merge](merge.md)
- [Concat-to-pairs conversion](convert.md)
- [Copy-number metadata](copy-numbers.md)

## Development

[Validation commands](development.md) and [documentation maintenance](documentation.md)
cover local development. Historical benchmark run records are not part of this
manual; synthetic benchmark scripts remain available as opt-in developer tools.
Batch and shard row targets are not strict process-memory limits. Consult each
API’s memory and ownership contract; no whole-genome throughput is guaranteed.
