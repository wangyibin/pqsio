# Parallel PQS writing

The parallel extension keeps ABI v1 and the existing pairs/concat schemas.
It is additive; existing synchronous writers and readers remain available.
No additional dependency is required. Polars remains 0.49.1.

## Flow and ownership

Multiple producers submit owned batches to a bounded, ordered input queue.
One coordinator validates complete batches and assembles numbered shards.
A fixed worker pool encodes and writes those shards concurrently. Results are
collected in shard order, and metadata is published only after every job succeeds.
Workers write unique files in the owner's `.partial` directory.

Each batch also ends a shard. A batch larger than `chunk_size` can produce
multiple shards; a complete concat read is never split, even if larger than the
shard target. Small batches therefore produce small files: batch near the shard
target when throughput matters. Output row order and metadata semantics match
the synchronous writer, but shard boundaries need not be identical.

`ParallelWriter` owns worker lifetime and publication. `Producer` is shareable
between threads and can outlive its writer; later submissions return errors.
Owner lifecycle operations must be exclusive. C/C++ producer destruction must
not overlap submissions on that handle. Python defers producer handle destruction
until active submissions finish. Rust producers are cloneable.

## Ordering and backpressure

Sequences are unique, contiguous integers starting at **0**, below `u64::MAX`.
They describe input order, not thread completion order. Concat read IDs must
increase within and across batches in that sequence order.

At most `queue_capacity` future batches may occupy the input queue. An extra
slot is reserved for the next expected sequence, so it can enter a full queue.
Submissions block when there is no available slot. Callers must keep the next
expected sequence schedulable: do not occupy every application worker with
future batches while leaving the expected batch unscheduled. Sequential inputs
can be assigned to producer threads in increasing order, or use fixed lanes
as demonstrated in `examples/parallel_bench.rs`.

`max_batch_bytes` limits the owned native allocation of one submitted batch:
record-vector capacity, string capacities and concat offsets. Oversized batches
are rejected before admission; split them between complete reads or explicitly
increase the limit. There are at most `workers` dispatched encoding jobs,
plus coordinator-local data and the bounded input queue. Completed results hold
only counts, not frames, and cannot accumulate without bound.

This is a bound on input buffering, **not a hard RSS limit**. DataFrame and
compression workspace, vector growth, contig dictionaries and allocator overhead
are additional. Caller-owned batches blocked on admission, C-to-Rust conversion,
and Python objects/ctypes arrays also use memory outside the accepted queue.
Bound the number of application producer threads and avoid materializing the
whole dataset in production. Total thread count includes Polars' own pool;
more encoding workers are not guaranteed to improve throughput.

## Completion and failures

A successful submit means queued, not validated or durable. Input validation,
worker I/O errors and panics are reported through later submissions or `finish()`.
A failure stops admission, wakes waiting submitters and prevents publication.
Workers are joined before staging cleanup. Dropping an unfinished owner aborts
and waits for workers; it does not publish partial output.

Join intended submitters before calling `finish()`. Finish closes admission,
checks for gaps among submitted sequences, waits for all jobs and publishes the
metadata/output. It cannot detect a final batch that the caller never submitted
and never declared. An empty dataset is valid. After finish, further submissions
and a second finish fail. As with the synchronous writer, publication does not
include fsync and is not a power-loss durability guarantee.

## Python

```python
from concurrent.futures import ThreadPoolExecutor
from pqsio import ParallelWriter

# Each item is a bounded list of Pair records in original input order.
# This small example materializes its inputs; stream/bound them in production.
with ParallelWriter("sample.pairs.pqs", {"chr1": 1_000_000},
                    workers=2, queue_capacity=4,
                    max_batch_bytes=64 * 1024 * 1024) as writer:
    with writer.producer() as producer:
        with ThreadPoolExecutor(max_workers=4) as pool:
            futures = [pool.submit(producer.write_batch, i, rows)
                       for i, rows in enumerate(batches)]
            for future in futures:
                future.result()
# The context manager calls finish only after submissions have completed.
```

For concat use `kind="concat"` and
`producer.write_reads(sequence, complete_reads)`, or
`producer.write_batch(sequence, alignments, read_offsets)`.
The Python encoding path still creates ctypes records; this is not zero-copy
and does not promise parallel Python-object conversion.

## Rust and native APIs

Rust: `ParallelWriter::create(..., ParallelOptions { workers, queue_capacity,
max_batch_bytes })`, then `writer.producer()`. Clone a producer per thread or
share it. Submit `producer.write_pairs(sequence, Vec<Pair>)` or
`producer.write_reads(sequence, Vec<Alignment>, Vec<usize>)`. Finish the owner.

C: `pqsio_parallel_open`, `pqsio_parallel_producer`,
`pqsio_producer_pairs` / `pqsio_producer_reads`, `pqsio_parallel_finish`,
`pqsio_producer_destroy` and `pqsio_parallel_destroy`. See `include/pqsio.h` for
pointer lifetimes. Error strings are thread-local: read them on the failing thread.

C++17: `pqsio::ParallelWriter`, `writer.producer()` and
`producer.write_pairs(sequence, rows)` / `write_reads(sequence, rows, offsets)`.
`tests/parallel.cpp` is a compiled multi-threaded example.

## Reproduction

Run from the pqsio directory:

```sh
pixi run --manifest-path ../cphasing-rs/pixi.toml cargo test --offline --locked --profile dev-release -j 4
pixi run --manifest-path ../cphasing-rs/pixi.toml cargo build --offline --locked --profile dev-release --lib --example parallel_bench -j 4
PQSIO_LIBRARY="$PWD/target/dev-release/libpqsio.so" PYTHONPATH=python:../CPhasing python -m unittest discover -s tests -p 'test_*.py' -v
pixi run --manifest-path ../cphasing-rs/pixi.toml cargo clippy --offline --locked --all-targets -- -D warnings
python scripts/run_parallel_bench.py
```

The runner uses 250,000 generated records per format, 25,000-record shards,
4 producers, 4 Polars/Rayon threads, up to 8 pinned CPUs, warmup and six measured
rounds with alternating order. Synchronous and 1/2/4/8-worker modes each verify
all fields at MAPQ 0, 1 and 20 and q0/q1 counts. Write time includes finish but
excludes data generation and verification. Peak RSS is sampled before reading
back, includes pre-generated inputs, and is not an isolated queue-memory metric.
Inputs and results stay in ignored `tests/output`; no real genome data is used.
