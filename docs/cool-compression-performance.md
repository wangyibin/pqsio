# Parallel pixel compression for pairs2cool

This follows the [full POJ profiling measurements](cool-poj-performance.md),
where serial HDF5/zlib compression was the largest remaining CPU cost.
The conversion now compresses pixel chunks in Rust workers and writes them
through a single HDF5 caller. No runtime dependency was added.

## Implementation

- Each job contains one chunk of three Int64 pixel columns: `bin1_id`,
  `bin2_id`, and `count`. Workers perform HDF5-compatible byte shuffle and
  zlib level-6 compression using the existing `flate2` dependency.
- The calling thread continues final pixel merging and index generation,
  then uses `H5Dwrite_chunk` to write compressed bytes. Only this thread holds
  HDF5 handles; every HDF5 call uses the crate's global lock.
- Chunks may complete out of order. Their assigned offsets preserve pixel
  order, and dataset extents grow monotonically. Partial final chunks have
  zero padding in physical storage and retain their true logical length.
- At most N jobs can be outstanding, counting queued, running and completed
  jobs. Raw pixel vectors are recycled. Workers are joined on success and
  normal error paths before the staging file is published or removed.
- HDF5 chunks retain `min(batch_rows, 65536)` elements. Single-thread runs,
  chunks smaller than 4,096 pixels and outputs shorter than a chunk retain
  ordinary HDF5 compression. Chromosome, bin and index compression is unchanged.

This preserves Cooler types, filters, level, counts, indexes and reader
compatibility. Compressed bytes are not guaranteed identical: offloaded
compression uses the existing `flate2` backend instead of HDF5's zlib build.
In the tested dependency graph, `gzp` enables `flate2`'s `zlib-ng` backend;
`flate2` selects it in preference to the also-enabled `zlib-rs` feature.
The measured change therefore includes both scheduling and encoder effects;
it is not an isolated measurement of parallelism with identical encoders.

## Full POJ results

For four workers and 4,000,000-record sorting chunks, three alternating runs
per version give:

| Metric | Serial pixel compression | Parallel pixel compression |
| --- | ---: | ---: |
| Median conversion time | 40.52 s | **14.88 s** |
| Conversion time range | 40.16–41.21 s | 14.72–14.96 s |
| Median process CPU time | 58.92 s | 44.66 s |
| Median peak RSS | 418.4 MiB | 435.7 MiB |
| Peak RSS range | 413.2–454.9 MiB | 400.4–443.6 MiB |
| Output size | 142,354,750 bytes | 143,627,801 bytes |

The median speedup is **2.72×**, or **63.3% less elapsed time**. CPU time
also falls, consistent with the encoder change as well as parallel scheduling.
Output grows by **0.89%** while retaining shuffle/gzip level 6. Peak RSS varies
with input/sort scheduling; bounded compression queues do not imply identical
process memory peaks.

All outputs match the preceding implementation's canonical pixel SHA-256:
`c92754a2ea6c127af4a903844ffda3f97ba61bfbf876d4eaba092bcc470c16f9`.

A single ten-worker comparison with 4,000,000-record chunks took **36.63 s
before and 12.92 s after**, with peak RSS **788.9 → 675.4 MiB** and process
CPU time **60.49 → 49.12 s**. This suggests further elapsed-time gains with
more workers, at higher memory/CPU cost than the four-worker configuration.
One run does not establish stable scaling or a memory reduction.

With the **default 1,000,000-record chunk**, a single four-worker comparison
took **44.47 s before and 19.00 s after**, with peak RSS **277.0 → 267.9 MiB**
and process CPU time **61.75 → 46.11 s**. The smaller chunk retains an
intermediate merge pass, explaining part of the difference from the larger
chunk configuration. Both versions in each comparison use the same chunk size.

For this input, a measured configuration is:

```sh
pqsio convert ../benchmark/poj/POJ.merge.pairs.pqs --mode pairs2cool \
  --bin-size 20k --min-mapq 1 --threads 4 --chunk-size 4000000 \
  -o POJ.20k.cool
```

Defaults remain one thread and 1,000,000-record chunks; parallel pixel
compression requires multiple threads. These results do not establish an
optimum for other inputs, resolutions, MAPQ thresholds, cold caches or machines.

### Separate CPU profile

One additional four-worker, 4,000,000-record-chunk conversion was sampled
with `perf` at 99 Hz and independently validated. Its timing is excluded from
all results above. Exclusive user-CPU samples were distributed approximately as:

| Threads | Share of CPU samples |
| --- | ---: |
| Four PQS reader/binning workers | 31.5% |
| Four `cool-deflate-*` workers | 27.2% |
| Four sorting workers | 24.2% |
| Calling thread, including native merge/HDF5 | 17.0% |

All four compression workers have similar shares (6.77–6.85% each), confirming
that compression work is distributed. These percentages describe sampled CPU
time, not elapsed-time fractions. The caller's `python` thread name includes
the Rust conversion and HDF5 work invoked through FFI.

Largest exclusive symbols include quicksort (15.8%), zlib-ng's
`longest_match_avx2` (11.1%), heap sift-down (7.5%) and `deflate_medium` (4.0%).
The host's older BFD cannot decode newer DWARF inline information reliably;
these results use flat ELF symbols/thread names with `--no-inline --call-graph
none`, rather than source-line or inclusive call-stack attribution. Profiling
artifacts are saved as `profile-new*` and `profile-*-flat.txt` alongside the
benchmark reports.

## Measurement method

Input is the unchanged `benchmark/poj/POJ.merge.pairs.pqs`, scanned directly
from its 214 q0 Parquet shards. All runs use 20 kb bins, MAPQ >= 1, and the
default 65,536-row output batches. The input contains 275,104,195 records;
113,367,238 contacts pass filtering, producing 67,788,006 nonzero pixels
across 501,239 bins on 13,044 contigs.

The baseline is the native library from the preceding POJ optimization,
with serial HDF5 pixel compression. Its pre-strip SHA-256 is
`8390f23152221177e972c2177daf3956062d1ff9b0ff2761a876d7240255a4d7`.
A local copy with debug symbols stripped is used for new baseline timings;
no executable code was changed. Both versions use Pixi's `dev-release` profile.
The measured stripped baseline SHA-256 is
`3daf369ee29a8c5f2f5d0098f337700889fbfc70d667898652b736cbc1e3f803`;
the new native library is
`b339ae09484461a8a32e12a1bf82596e61b0a8a2488002a75c2bfd399973dd9a`.

Measurements run sequentially on the same AMD EPYC 7713 host. Each conversion
is pinned to N CPUs for N workers. Polars/Rayon use N workers; unrelated
numerical-library pools are limited to one. The four-thread, 4,000,000-record
chunk comparison uses three runs per version, alternating order:
baseline/new, new/baseline, baseline/new. Additional configurations are single
runs and should be treated as exploratory.

Reported conversion wall time excludes Python/native library loading and
independent validation. It includes reading, sorting, scratch I/O, merging,
compression and file publication. Peak RSS is process `VmHWM`, including
native pools, excluding the separate verification process. CPU time sums all
process threads. Caches are not flushed; these are warm-input measurements,
not cold-storage throughput. Builds and tests do not overlap timed runs.

Each output is validated by an independent h5py process, which streams every
pixel, checks counts/order/bounds, full chromosome/bin tables and both indexes,
and hashes canonical little-endian Int64 pixel triplets. This is full
before/after equality and structural validation, not an independent
recalculation of all contacts. Small tests separately check binning semantics.
Metadata hashes and shard names/sizes/mtimes are checked before and after
each run; temporary `.cool` files and scratch runs are then removed.
All ten ordinary timed outputs passed. The local raw records are
`tests/output/pairs2cool-compression/{baseline,parallel}-t*-c*-r*.json`,
with a compact `summary.json` in the same directory.

## Reproduction and validation

Preserve the old library before building the new version. With the baseline
copy in the path shown below, one comparison is:

```sh
pixi run --locked cargo build --locked --profile dev-release -j 4
pixi run --locked python scripts/cool_pqs_bench.py \
  --input ../benchmark/poj/POJ.merge.pairs.pqs \
  --consumer-python /path/to/python-with-h5py-and-numpy \
  --library tests/output/pairs2cool-compression/baseline/libpqsio.so \
  --threads 4 --chunk-size 4000000 \
  --report tests/output/pairs2cool-compression/baseline-reproduction.json
pixi run --locked python scripts/cool_pqs_bench.py \
  --input ../benchmark/poj/POJ.merge.pairs.pqs \
  --consumer-python /path/to/python-with-h5py-and-numpy \
  --threads 4 --chunk-size 4000000 \
  --report tests/output/pairs2cool-compression/parallel-reproduction.json
```

Use a new report filename for each run and reverse baseline/new order in
alternate pairs. Reports include native-library checksums and the input
manifest. `source_sha256` records the workspace at benchmark invocation;
when supplying an older `--library`, it does not describe that older binary.
The saved baseline source copies and earlier report identify the old code.

Focused checks:

```sh
pixi run --locked cargo test --locked --profile dev-release -j 4 --lib cool::
PQSIO_COOLER_TEST_PYTHON=/path/to/python-with-cooler-h5py-numpy \
  pixi run --locked python -m unittest discover -s tests -p test_cool.py -v
pixi run --locked python -m unittest discover -s tests -p test_cli.py -v
pixi run --locked lint
```

The Rust checks include extreme Int64 values, full/partial/empty chunks,
forced out-of-order completion, bounded pending work, compression errors,
cancellation and a read-only HDF5 write failure. Independent Cooler/h5py
tests cover complete pixel arrays and indexes across multiple chunks, duplicate
counts, non-aligned transfer batches, and raw final-chunk decompression to
check shuffle layout and zero padding. No custom reader filter is required.
All **7 Rust tests, 10 Cooler integration tests and 11 CLI tests** passed,
including the independent consumer checks, as did Clippy with warnings denied.
