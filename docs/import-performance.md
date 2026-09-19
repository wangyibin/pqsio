# Native BAM/PAF import performance

All four BAM/PAF import modes now perform parsing, disk grouping, expansion and
PQS writing in Rust. The baseline is the **already optimized Python importer**
(column batches and SQLite scratch optimizations), not the earlier unoptimized
implementation. Both versions use the same new native library, isolating the
import implementation change from changes to the native Parquet writer dependencies.

## Results — 2026-09-19

Medians of three runs per version, with 80,000 input alignments in every case.
The machine is an AMD EPYC 7713 running Linux x86-64 and Python 3.11.16.
Workers are pinned to CPUs 0–3; Polars/Rayon use four threads. Import options
use one decoding/encoding worker, batch size 65,536 and shard target 1,000,000.

| Mode | Layout | Python (s) | Rust (s) | Speedup | Main-process peak RSS, Python → Rust (MiB) |
| --- | --- | ---: | ---: | ---: | ---: |
| `paf2pairs` | 4/read | 1.529 | 0.174 | 8.79× | 79.7 → 68.0 |
| `paf2pairs` | 32/read | 2.854 | 0.646 | 4.41× | 247.1 → 211.7 |
| `paf2pairs` | Scattered 4/read (gzip PAF) | 1.483 | 0.165 | 8.98× | 76.8 → 71.8 |
| `paf2concat` | 4/read | 1.801 | 0.167 | 10.81× | 112.2 → 62.2 |
| `paf2concat` | 32/read | 1.622 | 0.125 | 13.00× | 113.4 → 56.6 |
| `paf2concat` | Scattered 4/read (gzip PAF) | 1.793 | 0.187 | 9.57× | 111.8 → 64.0 |
| `bam2pairs` | 4/read | 1.913 | 0.133 | 14.34× | 77.4 → 68.0 |
| `bam2pairs` | 32/read | 3.348 | 0.637 | 5.26× | 239.4 → 209.1 |
| `bam2pairs` | Scattered 4/read (BAM) | 1.925 | 0.149 | 12.96× | 76.8 → 68.2 |
| `bam2concat` | 4/read | 2.490 | 0.154 | 16.18× | 112.1 → 62.7 |
| `bam2concat` | 32/read | 2.308 | 0.112 | 20.59× | 113.4 → 58.5 |
| `bam2concat` | Scattered 4/read (BAM) | 2.467 | 0.165 | 14.99× | 111.9 → 62.4 |

Each low/scattered pairs conversion produces **113,517 q0 pairs** and **68,112 q1 pairs**;
each high-order pairs conversion produces **1,173,040 q0 pairs** and **744,606 q1 pairs**.
Concat retains the filtered alignment records and original-read mapping instead
of expanding combinations. All **72 output datasets** matched the baseline and
passed full native validation.

## Method and correctness

- Each measurement runs in a fresh process, alternating baseline and candidate.
  Fixtures, decoded-output checks and native-library loading are outside the timer.
  Timed conversion includes decoding, grouping, output writing, publication and
  scratch cleanup. Filesystem caches are warm; tests/builds did not run concurrently.
- PAF fixtures include both strands, secondary alignments, MAPQ 0 and UInt64
  coordinates above 2³³. The plain layouts group records together; the scattered
  layout interleaves reads and compresses the PAF with ordinary gzip.
- BAM fixtures are generated from independent SAM records using samtools before
  timing. They include hard clips, reverse/supplementary/secondary alignments and
  NM tags. All BAM layouts use BGZF BAM with target coordinates below 2³¹.
  The legacy layout key `scattered-gzip` denotes scattered BAM for BAM modes.
- Order-sensitive SHA-256 digests cover every q0/q1 field, string lengths and
  string bytes, independent of batch/shard boundaries. Reference dictionaries,
  counts, options and statistics must match. Concat read-name sidecars are compared
  as parsed JSON records. Every dataset also passes `validate(level="full")`.
- Peak RSS is Linux `VmHWM`, recorded before output validation. For the BAM
  baseline it excludes the separate samtools process; it is a main-process
  measurement, not total process-tree memory. Reported CPU time similarly excludes
  the baseline decoder child and should not be compared as total CPU usage.
- High-order pair expansion remains quadratic. Parsing/sorting/expansion are
  native but currently serial. These small synthetic runs do not establish
  whole-genome, cold-cache, multi-worker, mgzip throughput or other-platform performance.

## Implementation

`src/import/` handles BAM/PAF parsing, CIGAR interpretation and bounded external
sorting. Sorting targets 32 MiB of records per run and merges at most 32 files
at once; complete read groups and PQS buffers add to process memory. Pair IDs
and numeric fields are packed into Rust column batches.

`src/io.rs` follows CPhasing common-reader/common-writer conventions with flate2
and gzp. It validates mgzip block framing before parallel decoding, checks gzip
CRC/truncation errors, and explicitly finishes compressed writers. BAM uses
rust-htslib directly. The Python importer is a single FFI call; no Python record
processing or decoder subprocess remains.

## Reproduce

The raw reports are retained locally as
`tests/output/rust-import-performance/{paf2pairs,paf2concat,bam2pairs,bam2concat}.json`.
The saved baseline package is `tests/output/rust-import-performance/python-baseline`.
These are ignored generated artifacts; a fresh checkout must supply the archived
baseline package separately. Example:

```sh
pixi run --locked bench-import \
  --baseline-python tests/output/rust-import-performance/python-baseline \
  --report tests/output/rust-import-performance/paf2pairs.json \
  --mode paf2pairs --alignments 80000 --repetitions 3
```

Repeat with `paf2concat`, `bam2pairs` and `bam2concat`. BAM benchmark fixture
generation and the historical Python baseline require samtools on PATH; the
current Rust importer does not. Raw JSON records all run times, fixture counts,
field digests, machine details and native/source SHA-256 values.

Recorded hashes:

- Baseline importer: `0c0ad704202459347884991b79c9c58a13ef09c55d398194b9a7b5ff898d00b1`
- Candidate Python binding: `219251277066b93e36094693ebd104741435bad76ee9b723a951c8b13ca7529e`
- Native library: `abd5f4818e389205aa8ed70be0b643ef81e4fcaaebd59c03f62c10210b93bd74`
- Benchmark script: `9678d6c965e61d81a5daa75d3a197d6eee335e301769ee27a300444c4b249ad3`

## Focused regression checks

- `pixi run --locked test-import`: 17 tests passed, including real BAM and an
  empty PATH, no Python writer/decoder calls, gzip/multi-member input, UInt64
  positions, clipping, mate handling, filtering, malformed input and cleanup.
- CLI unittest suite: 10 passed; existing conversion suite: 8 passed.
- Native C/C++ suite: 5 passed, including a C consumer of `pqsio_import_json`.
- Rust import test: forced multiple external-merge passes preserve order and ties.
- Rust I/O test: plain/gzip/mgzip round trips, standard gzip compatibility, CRC
  corruption and truncation checks passed.
- `pixi run --locked build` succeeded with the dev-release profile.
- Python syntax checks and `git diff --check` passed.

No real genome-scale pipeline, release-profile artifact, aarch64 execution or
documentation-site build was performed for this migration.
