# Native Rust CLI performance — 2026-09-20

100,000 synthetic pairs in 10 q0 shards (four 50 Mb contigs, 20% trans); PAF import uses 50,000 alignments from 25,000 two-alignment reads. Cooler resolution: 10 kb. All test data is temporary and removed after the run.

Each command runs in a fresh process, pinned to CPUs [0, 1, 2, 3]. POLARS_MAX_THREADS=4 and RAYON_NUM_THREADS=4; native worker counts are shown below. Build profile: `dev-release`. Values are medians of three repetitions, except version startup (ten). No cache eviction: these are local, warm-cache synthetic measurements, not cold-disk or POJ results.

The comparison is a minimal fresh Python process importing json/sys/pqsio and calling the same Rust core through the C ABI. It is **not** the historical rich-click CLI and does not measure a long-running Python application with the library already loaded. Launch/import/library-loading costs are included.

Wall time uses a monotonic high-resolution parent timer and pipe-based process completion. GNU time records process peak RSS; stdout/stderr are saved after timing. Output checking is outside the timed interval.

| Operation | Threads | Rust CLI, ms | Python API process, ms | Rust peak RSS, MiB | Python peak RSS, MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| version | 1 | 5.09 | 54.43 | 6.7 | 13.9 |
| info | 1 | 7.16 | 55.74 | 9.5 | 20.8 |
| head | 1 | 7.85 | 58.29 | 14.4 | 26.1 |
| stats | 1 | 36.45 | 85.44 | 23.0 | 34.5 |
| paf2pairs | 1 | 84.80 | 136.33 | 24.5 | 33.5 |
| paf2pairs | 4 | 96.19 | 137.28 | 23.1 | 34.8 |
| pairs2cool | 1 | 77.58 | 128.51 | 25.3 | 37.4 |
| pairs2cool | 4 | 46.50 | 99.95 | 25.2 | 37.8 |
| export | 1 | 77.40 | 126.05 | 23.2 | 34.9 |
| export | 4 | 75.84 | 119.95 | 23.5 | 34.8 |
| query_scan | 1 | 22.48 | 73.90 | 15.1 | 26.4 |
| query_indexed | 1 | 21.53 | 67.36 | 15.1 | 26.7 |

First query including automatic q0 index construction: **1.049 s** median (range 0.982–1.082 s). Each build uses a fresh sidecar directory with hard-linked immutable fixture files.

The indexed query returns the same 1,486 records while decoding 6 of 10 row groups (60,000 rather than 100,000 rows). Its median latency is nevertheless close to a sequential scan at this scale; index opening/checking offsets much of the decoding reduction. A one-off query on this fixture benefits from `--index off` or `--no-build-index`. Larger datasets and other region distributions may behave differently.

Native startup and short-command gains mainly come from removing Python startup/import/ABI-loading overhead. The underlying record algorithms are shared. Four-thread Cooler conversion is about 1.67× faster than one thread here; compression export is essentially unchanged and PAF import is slower with four threads on this small input. These three-repetition results do not establish large-data scaling.

Pairs/TSV content, previews, statistics and imported records matched across paths; an independent h5py process confirmed identical Cooler arrays/indexes and contact sums across paths and thread counts.

## Reproduce

```sh
pixi run build
pixi run python scripts/cli_bench.py \
  --report tests/output/browse/native-cli-performance.json \
  --consumer /path/to/python-with-h5py
```

The JSON report contains all samples, CPU time, RSS, query counters, affinity and system load. The benchmark script intentionally bounds fixture size; defaults are 100,000 pairs and three repetitions.
