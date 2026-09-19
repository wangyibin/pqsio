# Pairs to Cooler

`pairs2cool` converts pairs PQS or tab-delimited pairs text directly to a
single-resolution `.cool` file. Rust performs input decoding, binning, sorting,
pixel aggregation and HDF5 writing. Python only dispatches the native
call and receives its JSON summary; no `cooler`, `h5py` or external converter is
needed at runtime.

## Usage

```sh
# pairs PQS: use its reference dictionary and read q0 once.
pqsio convert sample.pairs.pqs --mode pairs2cool --bin-size 10k \
    --min-mapq 1 --threads 4 -o sample.10k.cool

# Plain, gzip or mgzip pairs text: use #chromsize headers when present.
pqsio convert sample.pairs.gz --mode pairs2cool --bin-size 10k \
    -o sample.10k.cool

# Without chromosome-size headers, supply chrom.sizes or a FASTA .fai.
pqsio convert sample.pairs --mode pairs2cool --bin-size 5k \
    --contigsizes genome.chrom.sizes --chunk-size 1000000 --batch-rows 65536 \
    --tmpdir scratch -o sample.5k.cool
```

The output and `<output>.partial` must not exist; output parents and any explicit
scratch directory must already exist. `--bin-size` (aliases `--binsize`, `-bs`)
is required. It accepts integer bp or CPhasing-style `k`, `m`, `g` suffixes,
case-insensitively: `10k` = 10000, `1m` = 1000000, `1.5M` = 1500000.
Suffixes use powers of 1000. Spaces are ignored and fractional bp after unit
conversion are truncated, as in CPhasing. The result must be in
`1..9223372036854775807`; zero, negative, nonfinite and overflow values fail.
The Python API accepts the same strings or integer bp. Input files need not be sorted.

```python
from pqsio import convert

result = convert("sample.pairs.pqs", "sample.10k.cool", mode="pairs2cool",
                 bin_size="10k", min_mapq=1)
print(result.to_dict())
```

The Rust API is `pqsio::pairs2cool(input, output, CoolOptions { bin_size: 10_000,
..Default::default() })`. C/C++ callers use `pqsio_pairs2cool_json` in `pqsio.h`.
The report includes `nchroms`, `nbins`, `nnz`, `sum`, `input_records`,
`skipped_mapq` and `skipped_unmapped`. `sum` is the number of accepted pairs;
`nnz` is the number of distinct occupied upper-triangle pixels.

## Input and counting rules

- Pair positions are **1-based**: chromosome-local bin index is
  `(position - 1) / bin_size`, rounded down. Chromosome order follows the PQS
  dictionary, supplied sizes file, or first appearance in `#chromsize` headers.
  The final bin on each chromosome ends at its declared length.
- PQS input must have pairs format. Only q0 is read; the q1 subset is never
  added again. `--min-mapq` filters the stored pair MAPQ. Parquet decoding
  projects only `chrom1`, `pos1`, `chrom2`, `pos2` and `mapq`; read IDs,
  strands and other columns are not decoded or validated by this conversion.
  Use `pqsio validate` when validation of the entire PQS dataset is needed.
- Text headers use `#chromsize: CHROM LENGTH` and optional `#columns:` names.
  Declared columns may be reordered but must contain `chrom1`, `pos1`, `chrom2`
  and `pos2`. Rows must match the declared width. Without `#columns`, rows must
  have the standard seven fields (`readID chrom1 pos1 chrom2 pos2 strand1
  strand2`), optionally followed by numeric MAPQ as the eighth field.
- Named quality columns may be `mapq` or both `mapq1` and `mapq2`; for two ends,
  the smaller quality is used. A positive `--min-mapq` requires MAPQ columns.
  Records with either chromosome `!` or `*` are skipped as unmapped. For
  retained records, unknown chromosomes and positions outside `1..length` fail.
- Sizes files contain at least two whitespace-separated columns (name, length)
  and can be compressed. Supplied sizes take precedence for chromosome order;
  headers must agree on names and lengths. Duplicate or late chromosome/column
  headers and malformed records fail with a line number. Chromosome names must
  be nonempty ASCII without whitespace or NUL; lengths must fit positive Int64.
- Endpoints are ordered by bin ID. Every accepted pair contributes one count,
  including diagonal contacts and duplicate records. Duplicate pixels are
  summed across all chunks; no read-level deduplication is performed.

`min_order`, `max_order`, `include_secondary`, `samtools` and `pair_position` do
not apply to this mode. PQS input uses its own dictionary, so `--contigsizes`
is accepted only for text input. Local files and directories are supported;
stdin and HDF5 group URIs are not.

## Output and resource use

Output follows the [Cooler v3 schema](https://cooler.readthedocs.io/en/latest/schema.html)
with `storage-mode=symmetric-upper`, fixed bins, sorted unique pixels, chromosome
offsets and CSR bin offsets. It contains raw counts only, without balancing
weights or `.mcool` resolutions. Chromosome names use fixed ASCII strings;
lengths, bin coordinates, pixel IDs/counts and offsets use Int64. Numeric
chromosome IDs use Int32. HDF5 datasets use gzip compression.

At most `--chunk-size` accepted pair records are sorted per run (default
1,000,000). If all accepted records fit in one run, including an exactly full
run, sorting and aggregation stay in memory and create no scratch run files.
Large in-memory runs can sort disjoint slices in parallel and merge their
aggregated pixels as HDF5 is written. Empty and fully filtered inputs use this
same path. The dense contact matrix is never allocated.

Larger inputs use sorted scratch runs. With `--threads N`, a bounded queue
feeds up to N sorting/writing workers while input decoding and binning continue.
Runs are merged with a maximum fan-in of 32, using more passes when necessary.
Final merging and all HDF5 API calls run on the calling thread. With multiple
threads, pixel chunks are byte-shuffled and zlib-compressed at level 6 by up
to N Rust workers while final merging continues. The caller writes these
compressed chunks directly to HDF5; workers never access HDF5 handles.
Completed chunks may arrive out of order and are written at their assigned
offsets. The last physical chunk is zero-padded without extending the logical
pixel count. The output retains standard shuffle/gzip filters and requires
no reader plugins. The existing `flate2` encoder can produce different
compressed bytes and file sizes from HDF5's zlib backend.

PQS input uses up to N workers to decode and bin independent Parquet row groups,
with bounded queues for row-group jobs and pixel batches. Parquet's internal
parallel decoding is disabled on these workers. `--batch-rows` (default 65,536)
controls pixel transfer batches and HDF5 buffers, not the Parquet row-group
decode size. Pixel compression jobs contain one HDF5 chunk of at most 65,536
pixels even for larger batches. One thread, chunks below 4,096 pixels, and
outputs smaller than one chunk use ordinary HDF5 compression. Chromosome,
bin and index datasets also retain ordinary HDF5 compression.
The decoder, sorter and compressor use separate pools; N is the limit
per stage, not a strict limit on total process threads. Small in-memory runs
are sorted on the calling thread to avoid worker startup overhead.

Memory increases with parallelism: the spill stage can hold N active and N
queued sorting runs, plus the current run. PQS decoding additionally retains
up to N decoded row groups and bounded pixel batches. Compression permits at
most N outstanding chunks, including queued, running and completed jobs,
plus the caller's current pixel buffer. Input vectors are recycled; each
running job also uses a shuffle buffer and three compressed column buffers.
Buffers can reserve more
space than their current record count. Reference dictionaries and HDF5 buffers
also consume memory; `--chunk-size`, `--batch-rows` and `--threads` are not
strict process-memory limits. Reduce the chunk size or thread count to lower
memory use.

`--threads` defaults to 1 and also controls mgzip decoding. Ordinary gzip uses
the shared Rust gzip reader; text parsing remains sequential, with sorting and
scratch writing overlapped when multiple runs are needed.

Scratch runs are created in a private subdirectory of `--tmpdir` or the output
parent. The HDF5 file is flushed and closed before publishing it with an atomic
hard link in the output directory; this requires a filesystem with hard-link
support and never replaces an existing output. Normal success/error paths
join worker threads before removing owned scratch and staging files. An abrupt
process termination can leave them behind. As with other native JSON APIs, a
callback failure after publication retains the completed output. Inputs must
remain unchanged during conversion.

## Validation

```sh
pixi run test-cool
# Optional independent reader for matrix/index/schema compatibility checks:
PQSIO_COOLER_TEST_PYTHON=/path/to/python-with-cooler-and-h5py pixi run test-cool
```

Tests cover empty and filtered output, reversed/duplicate/diagonal contacts,
chunk boundaries, compressed text, column mappings, invalid input and output
protection. Parallel checks cover reordered projected columns, multiple row
groups, corrupt/missing/null input, cancellation and scratch cleanup. Compression
tests cover full and partial chunks, out-of-order completion, bounded pending
work, worker/write errors and cancellation. The independent reader also
decompresses a raw final chunk to check its shuffle layout and zero padding.
Independent consumer checks include matrix values, both index
arrays, long chromosome names and Int64 coordinates. A Rust test exercises
aggregation across more than 32 squared sorting runs and multiple merge passes.

For the reproducible comparison with CPhasing's fast and low-memory modes,
see [pairs2cool performance](cool-performance.md).
For the projection, in-memory and pipeline comparison against the previous
Rust implementation, see [Rust optimization measurements](cool-optimization.md).
For the full 275-million-record POJ dataset and further profiling-driven
changes, see [POJ performance](cool-poj-performance.md).
For parallel pixel compression with one HDF5 writer, see
[compression measurements](cool-compression-performance.md).
