# pairs2cool on the full POJ dataset

This is a real-data follow-up to the [synthetic optimization measurements](cool-optimization.md).
It tests the existing `benchmark/poj/POJ.merge.pairs.pqs` dataset in place;
no input shards are copied, filtered into a new dataset, or modified.

The measurements below precede parallel pixel compression. See the
[compression follow-up](cool-compression-performance.md) for the subsequent
implementation and comparisons against this version.

## Input and verification

- q0: 275,104,195 records in 214 Parquet shards, occupying 2,967,037,252 bytes.
- MAPQ >= 1 retains 113,367,238 contacts; conversion still scans q0 once.
- At 20 kb: 13,044 chromosomes, 501,239 bins and 67,788,006 nonzero pixels.
- Output: 142,354,750 bytes, raw symmetric-upper counts, Int64 pixel columns,
  65,536-element HDF5 chunks and gzip level 6, as before.

Every output is streamed through an independent h5py process. The checker
validates all chromosome names/lengths and bin boundaries, both complete index
arrays, sorted unique upper-triangle pixels, positive counts, metadata and total
counts. Full canonical Int64 pixel triplets are SHA-256 hashed to compare all
thread settings and native versions. The expected input/accepted totals also
match the dataset's recorded q0/q1 counts.

This combines structural/count validation with exact before/after pixel
equality; it is not an independent recomputation of every contact's bin.
Independent binning and boundary oracles are covered by the small Cooler tests.
Historical POJ `.cool` files and CPhasing timings are not controls here: they
used a different input path, and CPhasing's older fast binning convention can
differ at exact bin boundaries.

## Findings and implementation

A separate `perf record` run sampled user CPU time at 99 Hz. It is excluded
from ordinary timing comparisons. Before this change, the largest sampled
symbols included:

| Symbols | Approximate share of CPU samples |
| --- | ---: |
| zlib `deflate_slow` + `longest_match` | 29.8% |
| `ChunkedArray::get` + `Numbers::at` + `Codes::at` | 13.7% |
| heap `pop` + `Merged::pop` | 12.3% |
| unstable quicksort | 7.1% |

These are exclusive CPU sample shares, not percentages of elapsed wall time.
The main thread is labelled `python` by perf, but its conversion work is Rust
and HDF5/zlib inside the native library; that label does not indicate Python
record processing.

The resulting small native changes are:

- Borrow contiguous non-null numeric and categorical index buffers once per
  row group, avoiding repeated Polars chunk searches per cell. Nullable or
  multi-chunk columns keep the existing checked fallback; dictionary lookup
  failures and Int64-range coordinate support are preserved. No unsafe buffer
  access is introduced.
- Replace the minimum heap entry during sorted-run merging with `peek_mut`,
  requiring one heap adjustment instead of a separate pop and push. This also
  applies to in-memory parallel sort prefixes.

HDF5 compression, output types, coordinate semantics and filtering are unchanged.

## Initial full-dataset timing sweep

Each table entry is **one full conversion**, not a median or a confidence
interval. Baseline configurations ran in 1/4/10-thread order, then optimized
configurations ran in reverse order. Small differences need repeat runs before
being treated as stable speedups.

With the default 1,000,000-contact sorting chunks:

| Threads / CPUs | Before (s) | After (s) | Time reduction | Peak RSS, before → after (MiB) |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 67.74 | 64.21 | 5.2% | 71.9 → 73.1 |
| 4 | 51.46 | 43.47 | 15.5% | 249.4 → 277.4 |
| 10 | 44.74 | 43.49 | 2.8% | 373.3 → 425.5 |

The four-thread result approaches the ten-thread result in this sweep, with
lower memory and CPU usage (58.06 versus 62.70 CPU seconds). Ten workers are
not established as faster for this input. The default remains one worker;
parallel workers are optional and increase in-flight buffers.

All six outputs share the canonical pixel SHA-256:
`c92754a2ea6c127af4a903844ffda3f97ba61bfbf876d4eaba092bcc470c16f9`.

### Sorting chunk tradeoff

A further single full conversion with the optimized library, four threads and
`--chunk-size 4000000` took **38.40 s**, with **469.3 MiB** peak RSS and 54.88 CPU
seconds. It produced the same full pixel checksum. This reduces elapsed time by
11.7% relative to the optimized four-thread default chunk, or 25.4% relative to
the initial four-thread implementation, while consuming more memory.

The 113,367,238 accepted contacts require 114 sorting runs with 1,000,000-record
chunks, but only 29 with 4,000,000-record chunks. With a merge fan-in of 32, the
latter fits directly into the final merge and avoids one intermediate pass.
The default chunk size is unchanged because the memory/throughput tradeoff is
input dependent. A useful starting configuration for this POJ dataset is:

```sh
pqsio convert ../benchmark/poj/POJ.merge.pairs.pqs --mode pairs2cool \
  --bin-size 20k --min-mapq 1 --threads 4 --chunk-size 4000000 \
  -o POJ.20k.cool
```

This is an observed configuration, not a global optimum. It has not been tested
at other resolutions, MAPQ thresholds, on another machine, or with cold caches.

### Remaining optimization opportunities

A final separate profile of the optimized four-thread, 4,000,000-record-chunk
configuration places **43.2% of CPU samples** in `deflate_slow` and
`longest_match` alone. Quicksort accounts for 10.3%, heap sift-down plus
`Merged::pop` for 7.2%, and the numeric/categorical accessors for 4.3%.
The before/after profiles use different thread and chunk settings, so their
percentages do not isolate the effect of individual code changes.

In the version measured here, HDF5 compression still runs on the writing thread.
The subsequent [compression follow-up](cool-compression-performance.md) implements
and measures the proposed next experiment:
parallel chunk compression feeding a single HDF5 writer, preserving the shuffle
filter, gzip level and complete/partial-chunk semantics. For smaller sorting
chunks, independent intermediate-merge groups remain an opportunity for
concurrency; they would need separate throughput, memory and correctness checks.

## Method and reproduction

Runs use the AMD EPYC 7713 host, Python 3.11.16 and Rust's `dev-release` profile.
The process is pinned to CPUs 0 through N-1 for N threads, with matching
Polars/Rayon limits and one BLAS/OpenMP/NumExpr thread. Conversion is sequential
across configurations; compilation and tests do not overlap timed conversion.
There is no cache flushing; ordinary timings follow a full profiling run and
use warm filesystem caches. No durable disk flush timing is claimed.

The native timer starts after explicit module/library loading and includes
decoding, filtering, sorting, intermediate merges, compression, publication and
scratch cleanup. Reports also retain full-process wall time and CPU time.
Peak RSS is process `VmHWM`, including all native worker threads and excluding
the filesystem page cache. These measurements are not memory caps. Output
verification is separate and untimed. Each full `.cool` and its scratch files
are removed after verification; only reports, logs and profiles are retained.

Archive the previous library before rebuilding, then use the same input:

```sh
mkdir -p tests/output/pairs2cool-poj/baseline
pixi run --locked objcopy --strip-debug target/dev-release/libpqsio.so \
  tests/output/pairs2cool-poj/baseline/libpqsio.so
pixi run --locked python scripts/cool_pqs_bench.py \
  --input ../benchmark/poj/POJ.merge.pairs.pqs \
  --consumer-python /path/to/python-with-h5py-and-numpy \
  --library tests/output/pairs2cool-poj/baseline/libpqsio.so \
  --threads 1 4 10 --report tests/output/pairs2cool-poj/baseline.json
# Apply the native changes, build and test, then omit --library to time them.
pixi run --locked build
pixi run --locked python scripts/cool_pqs_bench.py \
  --input ../benchmark/poj/POJ.merge.pairs.pqs \
  --consumer-python /path/to/python-with-h5py-and-numpy \
  --threads 1 4 10 --report tests/output/pairs2cool-poj/optimized.json
```

`--profile --threads 10` runs a separate sampled conversion and saves a
`.perf.data` file alongside its report. Input provenance records hashes of the
small metadata/dictionary files plus every q0 shard's name, size and timestamp;
these are checked again after each run. Full input shard content is not hashed.
Native libraries, benchmark source and native conversion source are hashed.
Reports must be written under the repository's ignored `tests/output` directory.

Reports are in `tests/output/pairs2cool-poj`: `baseline.json`, `optimized.json`
and `optimized-chunk4m.json` hold the ordinary timings; `profile-before.json`
and `profile-after.json` are separate profiling runs. To reproduce the larger
chunk measurement, add `--threads 4 --chunk-size 4000000` and choose a new report
path. Existing reports are never overwritten by the script.
`summary.json` confirms unchanged input manifests and identical canonical pixel
hashes across all **nine** full conversions (seven ordinary runs and two
profiling runs).

The timed baseline library hash is
`cd1be0cef20c08467da7abdbc7b68f992991a8072aed20cdbb7c28f26f17350e`;
the optimized library hash is
`8390f23152221177e972c2177daf3956062d1ff9b0ff2761a876d7240255a4d7`.

## Focused validation

These commands passed: 5 Rust tests, 9 Cooler integration tests and Clippy with
warnings denied. The added unit test checks borrowed numeric values at UInt8,
UInt32 and UInt64 widths, null handling, multi-chunk fallback and invalid
categorical dictionary references. Existing tests exercise sorted-run merging
over more than 32² runs and compare Cooler matrices with an independent reader.

```sh
pixi run --locked cargo test --locked --profile dev-release -j 4 --lib cool::
PQSIO_COOLER_TEST_PYTHON=/path/to/python-with-cooler-and-h5py \
  pixi run --locked python -m unittest discover -s tests -p test_cool.py -v
pixi run --locked lint
```
