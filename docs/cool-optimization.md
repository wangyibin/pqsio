# Rust pairs2cool optimization measurements

This compares the previous Rust converter with the projection, in-memory and
parallel pipeline implementation on the same machine. The earlier
[CPhasing comparison](cool-performance.md) records a separate historical run.

## Implementation

- PQS reads only `chrom1`, `pos1`, `chrom2`, `pos2` and `mapq` from q0. Numeric
  columns are accessed through decoded column buffers, without creating full
  pair records or copying read IDs and strands. Row-group jobs share no file
  seek position; job construction avoids repeatedly cloning the full footer's
  row-group list.
- Up to `chunk_size` accepted contacts, including an exactly full chunk, stay
  in memory. Duplicate pixels are aggregated before HDF5 writing; unused vector
  capacity is released. Large chunks sort disjoint slices in parallel and merge
  their aggregated prefixes directly, without binary scratch runs.
- For larger inputs, separate bounded queues overlap PQS row-group decoding
  and binning with sorting and scratch writing. Worker errors propagate to the
  caller, workers are joined, and owned temporary files are removed. Final
  merging and HDF5 writing remain on one thread. Text parsing and ordinary gzip
  decoding remain sequential.

`--threads N` limits workers **per stage**, not the sum of process threads.
Increasing N can increase RSS through simultaneous Parquet row groups, queued
pixels and sorting runs. See [resource use](cool.md#output-and-resource-use).
The API, coordinates, MAPQ filtering and reported counts are unchanged.

## Results — 2026-09-19

All 72 measured outputs, 24 warmup outputs and 12 separate boundary probes
matched the independent oracle. The tables show median conversion times from
three measured runs per combination. These compare the combined changes, not
isolated contributions from each optimization.

With the default `--chunk-size 1000000`, the optimized converter keeps all
800,000 accepted contacts in memory:

| Input | Threads / CPUs | Previous (s) | Optimized (s) | Speedup | Peak RSS, previous → optimized (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| pairs | 1 | 0.473 | 0.460 | 1.03× | 45.5 → 46.3 |
| pairs | 4 | 0.457 | 0.426 | 1.07× | 46.3 → 46.8 |
| gzip | 1 | 0.534 | 0.494 | 1.08× | 46.2 → 46.7 |
| gzip | 4 | 0.520 | 0.491 | 1.06× | 46.3 → 47.1 |
| PQS | 1 | 0.302 | 0.259 | 1.17× | 64.6 → 54.4 |
| PQS | 4 | 0.300 | 0.209 | 1.44× | 64.2 → 66.3 |

PQS benefits most: the four-thread in-memory path is 1.44× faster than the
previous converter and 1.24× faster than the optimized single-thread path.
Text/gzip improvements are smaller because parsing and ordinary gzip decoding
remain sequential. Some subsecond samples fluctuate: optimized single-thread
PQS spans 0.237–0.276 s and four-thread PQS spans 0.209–0.229 s. The raw reports
retain every sample; small ratios should not be extrapolated to real datasets.

With `--chunk-size 100000`, both converters spill eight runs:

| Input | Threads / CPUs | Previous (s) | Optimized (s) | Speedup | Peak RSS, previous → optimized (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| pairs | 1 | 0.482 | 0.472 | 1.02× | 36.9 → 36.5 |
| pairs | 4 | 0.501 | 0.430 | 1.16× | 36.8 → 38.1 |
| gzip | 1 | 0.536 | 0.545 | 0.98× | 36.9 → 36.8 |
| gzip | 4 | 0.536 | 0.481 | 1.11× | 36.9 → 38.3 |
| PQS | 1 | 0.324 | 0.273 | 1.18× | 48.0 → 42.9 |
| PQS | 4 | 0.322 | 0.222 | 1.45× | 48.3 → 62.3 |

The PQS pipeline is 1.45× faster than the old four-thread setting, at the cost
of about 14 MiB additional peak RSS. Its four-thread measurements span
0.214–0.240 s. Single-thread gzip shows no improvement in this run. The
four-thread queue improves overlap rather than reducing all workloads' memory;
choose thread and chunk settings according to the available memory.

## Measurement method

Both versions run the same Python CLI and native ABI, using an archived copy
of the previous native library for `pqsio-baseline`. The archive was made with
`objcopy --strip-debug` before rebuilding; executable code and exported ABI
were preserved. No CPhasing conversion is timed in this comparison. Its existing
Python environment supplies independent Cooler/h5py verification.

The deterministic fixture contains 1,000,000 input pairs, 800,000 accepted
contacts, 316,667 distinct pixels and 4,000 bins at 10 kb. Plain pairs, ordinary
gzip and PQS represent the same records. PQS has ten 100,000-row q0 shards;
q1 is present but is not read. Bin-boundary coordinates are checked separately.
The default 1,000,000-record chunk fits all accepted records in memory; the
100,000-record chunk requires eight sorting runs and exercises the pipeline.

Each backend/input/thread/chunk combination has one warmup and three measured
fresh-process runs, with alternating backend order. Runs are sequential, with
warm filesystem caches. Timers exclude explicit module/native-library loading,
fixture creation and output verification, and include conversion, compression,
publication and first-use runtime initialization. JSON also records end-to-end
process time. Every output is compared to an independent oracle using all
pixels, bins, both indexes and key attributes.

The host is AMD EPYC 7713, Linux x86-64; workers are pinned to CPU 0 or CPUs 0–3.
Both converters use Python 3.11.16 and Rust `dev-release` builds. Polars/Rayon
limits match the CPU budget; BLAS/OpenMP/NumExpr limits are one. Memory is main
process `VmHWM`, including imported libraries and all native worker threads.

## Reproduce

Preserve the old native library **before** building the new implementation:

```sh
mkdir -p tests/output/pairs2cool-optimization/baseline
pixi run --locked objcopy --strip-debug target/dev-release/libpqsio.so \
  tests/output/pairs2cool-optimization/baseline/libpqsio.so
# Apply the implementation changes, then rebuild.
pixi run --locked build
pixi run --locked python scripts/cool_bench.py \
  --baseline-python /path/to/python-with-cooler-and-h5py \
  --cphasing-source /path/to/CPhasing \
  --baseline-library tests/output/pairs2cool-optimization/baseline/libpqsio.so \
  --backends pqsio-baseline pqsio --rows 1000000 --threads 1 4 \
  --report tests/output/pairs2cool-optimization/final-in-memory.json
```

Repeat the final command with `--chunk-size 100000` and a separate
`final-spilled.json` report. Fixtures and Cooler files are temporary; reports,
logs and the archived library remain in ignored `tests/output`. The script
records native/source hashes, CPU affinity, software versions, per-run timings,
memory and correctness digests. It adds no runtime dependencies.

The final reports are `tests/output/pairs2cool-optimization/final-in-memory.json`
and `tests/output/pairs2cool-optimization/final-spilled.json`. Earlier exploratory
reports in that directory are excluded from the tables. Both final reports
record these SHA-256 hashes:

- Previous archived native library: `997baa52914cadf71804f6a5efe010dabb0342f5beb8be0f2d9c2701edfb89d1`
- Optimized native library: `25eb9b66906a0085cf46cba09ee739d2d996525706f35e26eedb5d1d348a5544`
- Benchmark script: `ad7322d89a9e9b44c5b6415e4c7d95f2a8bda4368f8ef8b05e6239fa7d04aeb1`

After these measurements, a diagnostic-only fix prevented producer cancellation
from masking a decoder's original shard error; the focused correctness tests
were rerun. The timed library precedes that error-path fix. The successful
conversion pipeline is unchanged, and no additional speedup is claimed for it.

## Correctness and limits

Validation commands for this implementation:

```sh
pixi run --locked cargo test --locked --profile dev-release -j 4 --lib cool::
PQSIO_COOLER_TEST_PYTHON=/path/to/python-with-cooler-and-h5py \
  pixi run --locked python -m unittest discover -s tests -p test_cool.py -v
pixi run --locked python -m unittest discover -s tests -p test_cli.py -v
pixi run --locked python -m unittest discover -s tests -p test_native.py -v
pixi run --locked lint
```

The 4 Rust, 9 Cooler, 11 CLI and 5 C/C++ tests passed, as did Clippy with warnings
denied. Tests cover projected columns, reordered columns and multiple row
groups; 1/2/4 threads; exact chunk capacity; empty/filtered inputs; coordinates
at bin boundaries and above UInt32; duplicate aggregation over more than 32²
scratch runs; corrupt footers, missing/null columns, count overflow, failed
scratch writes, cancellation and cleanup. Cooler independently reads the schema,
matrix and indexes. The conversion itself uses Rust for data I/O and processing.

These are small synthetic, warm-cache measurements. They do not establish
real-genome, cold-cache or mgzip throughput. The eight-run benchmark does not
measure multiple external-merge passes at scale; that behavior is covered by
the focused correctness test. HDF5 compression and final merging limit parallel
speedup. Chunk sizes and thread counts are not strict memory caps.
