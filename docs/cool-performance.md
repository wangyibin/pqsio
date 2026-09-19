# pairs2cool performance against CPhasing

This benchmark compares the current local CPhasing checkout with pqsio's Rust
converter before the projection, in-memory sorting and pipeline optimizations.
For the subsequent Rust changes, see [optimization measurements](cool-optimization.md).
Both consume identical records and chromosome dictionaries, apply
MAPQ >= 1, and write unbalanced, single-resolution Cooler files at 10 kb.

## Results — 2026-09-19

All **108 measured outputs** matched the independent oracle, as did 36 warmup
outputs. The filtered fixtures contain 80,000 / 800,000 contacts and
57,665 / 316,667 nonzero pixels respectively, across 4,000 bins. Plain input
sizes are 4.12 / 42.22 MB, gzip sizes 1.17 / 11.76 MB, and PQS Parquet sizes
(q0 + q1) 1.32 / 12.85 MB.

Median command-invocation time over three runs, excluding explicit dependency
preloading but including first-use runtime initialization:

| Input records | Input | CPU budget | CPhasing fast (s) | pqsio (s) | Speedup | Main-process peak RSS, CPhasing → pqsio (MiB) |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 100,000 | pairs | 1 | 0.778 | 0.079 | 9.85× | 235.0 → 33.7 |
| 100,000 | pairs | 4 | 0.767 | 0.078 | 9.84× | 262.2 → 33.6 |
| 100,000 | gzip | 1 | 0.784 | 0.084 | 9.33× | 237.0 → 33.9 |
| 100,000 | gzip | 4 | 0.770 | 0.083 | 9.22× | 262.5 → 34.0 |
| 100,000 | PQS | 1 | 0.764 | 0.064 | 11.88× | 239.6 → 41.3 |
| 100,000 | PQS | 4 | 0.754 | 0.064 | 11.70× | 275.1 → 41.4 |
| 1,000,000 | pairs | 1 | 1.370 | 0.470 | 2.92× | 344.3 → 45.6 |
| 1,000,000 | pairs | 4 | 1.161 | 0.469 | 2.48× | 418.0 → 45.4 |
| 1,000,000 | gzip | 1 | 1.450 | 0.522 | 2.78× | 354.1 → 45.6 |
| 1,000,000 | gzip | 4 | 1.245 | 0.522 | 2.39× | 421.9 → 45.2 |
| 1,000,000 | PQS | 1 | 1.043 | 0.309 | 3.38× | 320.7 → 62.9 |
| 1,000,000 | PQS | 4 | 0.875 | 0.319 | 2.74× | 375.1 → 64.0 |

For the million-record, four-CPU cases, pqsio uses **83–89% less main-process
peak RSS**. Its four-thread setting gives no material improvement for these
ordinary-gzip/text/PQS workloads; sorting and HDF5 output remain sequential.
Small-input ratios include substantial fixed first-call costs and should not
be extrapolated as sustained-throughput speedups. The million-row pqsio PQS
four-CPU samples span 0.308–0.363 s (median 0.319 s); all per-run values remain
in the raw report.

CPhasing's explicit low-memory mode, on the million-record inputs:

| Input | CPU budget | CPhasing low-memory (s) | pqsio (s) | Speedup | Main-process peak RSS, CPhasing → pqsio (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| pairs | 1 | 1.776 | 0.470 | 3.78× | 310.7 → 45.6 |
| pairs | 4 | 1.540 | 0.469 | 3.28× | 325.8 → 45.4 |
| gzip | 1 | 1.870 | 0.522 | 3.58× | 311.0 → 45.6 |
| gzip | 4 | 1.576 | 0.522 | 3.02× | 322.4 → 45.2 |
| PQS | 1 | 1.904 | 0.309 | 6.17× | 302.6 → 62.9 |
| PQS | 4 | 1.805 | 0.319 | 5.65× | 338.3 → 64.0 |

Fresh-process wall time including Python/module/native-library startup and
shutdown, for one million input records and four CPUs:

| Input | CPhasing fast (s) | pqsio (s) | Speedup |
| --- | ---: | ---: | ---: |
| pairs | 2.269 | 0.565 | 4.01× |
| gzip | 2.368 | 0.616 | 3.85× |
| PQS | 1.969 | 0.465 | 4.23× |

These are different implementations with their existing storage choices, not
a test that isolates programming-language overhead. Both use gzip level 6 in
HDF5, but pqsio uses Int64 pixel IDs/counts and 65,536-element chunks; CPhasing
uses UInt16 IDs, Int32 counts and smaller chunks on this fixture. The million-row
output occupies about 442 kB for pqsio versus 457 kB for CPhasing.

Environment: AMD EPYC 7713, Linux x86-64; pqsio uses Python 3.11.16 and its
`dev-release` Rust build. CPhasing uses the existing Python 3.8.8 environment
with Cooler 0.9.1, h5py 3.8.0, Polars 0.20.29 and NumPy 1.20.1. This compares
the working environments available on this machine, not identical Python
environments. CPhasing HEAD is `a7ed23275a20168237ef510a6bbbbf56fbebea75`
with pre-existing local modifications to `cli.py` and `pqs.py`; their exact
source hashes are recorded. Neither converter was modified during this test.

## Method

- Deterministic synthetic inputs contain 100,000 or 1,000,000 pairs across eight
  5 Mb chromosomes. They mix cis/trans contacts, diagonal contacts, duplicate
  records and MAPQ 0/10/30/60. Twenty percent are filtered out. Each fixture is
  encoded as plain pairs, ordinary gzip pairs and pairs PQS (100,000 rows/shard).
- Each backend/format/CPU-budget combination gets one untimed warmup and three
  measured runs in fresh processes, with alternating backend order. Measurements
  run sequentially, with warm filesystem caches. Fixtures and verification are
  outside the timer; temporary input/output data is removed afterward.
- Workers are pinned to CPU 0 for one-thread runs or CPUs 0–3 for four-thread
  runs. Polars and Rayon pools have the same limit; BLAS/OpenMP/NumExpr are set
  to one thread. Child processes inherit CPU affinity. An option named
  `--threads` does not make every conversion stage parallel.
- The conversion timer wraps command invocation after explicit module/native
  library preloading. It includes command setup, decoding, binning, aggregation,
  HDF5 writes, publication, and any remaining first-use runtime initialization.
  A separate process wall timer includes interpreter/module/library startup and
  shutdown. Neither timer includes fixture creation or output verification.
- Peak memory in the tables is Linux `VmHWM` for the **main process**, sampled
  before verification. It includes imported libraries. It excludes the separate
  `cphasing-rs pairs-filter` child used by CPhasing's text low-memory path.
  GNU time maximum RSS is also retained in JSON; it is not the simultaneous sum
  of process-tree memory. CPU time likewise describes the main process only.
- CPhasing is tested with explicit `--no-low-memory` (fast) and `--low-memory`.
  Its automatic mode choice is excluded. pqsio uses a 1,000,000-record sort-run
  target and 65,536-row batches. CPhasing's existing defaults for aggregation and
  HDF5 storage are preserved. PQS MAPQ filtering reads q1 in CPhasing and q0 in
  pqsio; both must select the same records.
- Independent h5py reads verify chromosome dictionaries, bin boundaries,
  chromosome/bin offsets, every sorted pixel triplet, counts and key attributes
  against a fixture-derived oracle. Pixel comparisons use canonical Int64
  values, allowing the implementations' different physical HDF5 integer types.

## Coordinate boundary difference

The local CPhasing fast paths (text and PQS), and its PQS low-memory path, use
`pos // binsize`. pqsio uses `(pos - 1) // binsize` for 1-based pairs positions.
CPhasing's text low-memory path delegates to Cooler and agrees with pqsio.

A separate three-record probe on a 25 bp chromosome, with bin size 10, exercises
positions exactly on bin boundaries. This is a correctness probe, not a timing
fixture. For example, a pair at positions 1 and 10 contributes to pixel `(0, 0)`
in pqsio and CPhasing's text low-memory mode, but `(0, 1)` in the other CPhasing
paths. Timing fixtures deliberately avoid exact bin multiples so equal matrices
can be required without changing either implementation. This does not establish
equivalence for arbitrary real input.

## Reproduce

From the pqsio repository, using an existing CPhasing environment:

```sh
pixi run --locked build
pixi run --locked python scripts/cool_bench.py \
  --baseline-python /path/to/cphasing/python \
  --cphasing-source /path/to/CPhasing \
  --report tests/output/pairs2cool-performance/report.json
```

The baseline environment must import CPhasing, Cooler and h5py. The local
`CPhasing/bin/cphasing-rs` executable is used by its low-memory text path.
The benchmark adds no pqsio runtime dependencies and leaves both converters
unchanged. The JSON records every run, correctness digests, HDF5 layout,
versions, machine information and source/native-library hashes. Generated
reports stay in ignored `tests/output`; the benchmark script and this document
are source files.

The local complete report is
`tests/output/pairs2cool-performance/report.json`; the initial 1,000-row harness
smoke report is `tests/output/pairs2cool-performance/smoke.json` and is excluded
from the performance tables. Recorded full-run hashes:

- Native library: `07629ee0bed0f98d2850c45efe4807b08efa5fe6a7db3e3777cfdff9c76f8368`
- Benchmark script: `f138ad5a05936dec9d28616b80d0c4dcebf927ca3acad0825cf480358f47e6cd`
- CPhasing Rust executable: `a8f51a2856a1d32838f3a73dc4f9453e32bfe1330f8dee320fb2ca1aefd0bdd8`

These runs measure small synthetic, warm-cache workloads on one Linux x86-64
host. They do not establish real-genome, cold-cache, mgzip, multi-resolution,
balancing or other-platform performance, and do not measure durable disk flush
latency. Further tests should use representative real datasets before choosing
production memory limits or estimating multi-billion-contact run times.
At these sizes the accepted pairs fit in one default pqsio sorting run; this
benchmark does not measure the throughput of multiple external-merge passes.
