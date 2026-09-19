# API overview

The Python API and CLI dispatch dataset operations to the native Rust library.
Install/build instructions are in [installation](installation.md). The Python
storage binding uses the standard library; the CLI uses `rich-click`, installed
with the Python package. Conversion does not require Python
`pysam`, `pyarrow`, `h5py`, `cooler`, or a `samtools` subprocess.

## Choose an entry point

| Operation | Python | Rust | C ABI |
| --- | --- | --- | --- |
| PQS summary | `info(..., stats=False)` | `presentation::info` | `pqsio_info_json` |
| Quality statistics | `stats(..., min_mapq=0)` | `statistics::stats` | `pqsio_stats_json` |
| Index status | `index_status(..., quality="q0")` | `query::index_status` | `pqsio_index_status_json` |
| PQS text preview/export | `view`, `export` | `presentation::export`, `ExportOptions` | `pqsio_export_json` |
| Concat PQS → pairs PQS | `convert(..., mode="concat2pairs")` | `convert(..., "concat2pairs", ConvertOptions)` | `pqsio_convert_json` / `pqsio_convert_parallel_json` |
| BAM/PAF → pairs or concat PQS | `convert(..., mode=...)` | `import_alignments(..., mode, ImportOptions)` | `pqsio_import_json` |
| Pairs PQS/text → Cooler | `convert(..., mode="pairs2cool", bin_size=...)` | `pairs2cool(..., CoolOptions)` | `pqsio_pairs2cool_json` |
| PQS metadata and validation | `inspect`, `validate` | `inspect`, `validate` | `pqsio_inspect_json`, `pqsio_validate_json` |
| PQS filtering and merging | `subset`, `merge` | `subset`, `merge` | `pqsio_subset_json`, `pqsio_merge_json` |

Python's public conversion entry point is **`pqsio.convert`**. Rust's
`pqsio::convert` accepts only `concat2pairs`; imports and Cooler conversion have
separate Rust functions. C++ callers can use these C entry points directly;
the C++ header does not currently provide conversion member functions.

See [Python signatures](python.md), [Rust examples](rust.md),
[C/C++ callbacks](native.md), and the [CLI reference](cli.md).
Streaming, columnar and query APIs are documented separately under
[streaming](streaming.md), [columnar I/O](columnar.md), [parallel writing](parallel.md)
and [region queries](query.md). Region queries and indexes have dedicated CLI
commands; copy-number operations remain API-only. See [quality statistics](stats.md)
for metric definitions.

## Conversion modes and coordinates

| Mode | Input | Output | Position convention |
| --- | --- | --- | --- |
| `concat2pairs` (default) | concat PQS directory | pairs PQS directory | 1-based reference midpoint |
| `bam2pairs` | Hi-C or long-read BAM | pairs PQS directory | 1-based leftmost by default; optional aligned 5′ end |
| `bam2concat` | Hi-C or long-read BAM | concat PQS directory | 0-based half-open alignment intervals |
| `paf2pairs` | PAF, gzip/mgzip PAF | pairs PQS directory | Same pair-position choices as BAM |
| `paf2concat` | PAF, gzip/mgzip PAF | concat PQS directory | 0-based half-open alignment intervals |
| `pairs2cool` | pairs PQS directory or pairs text, optionally gzip/mgzip | single-resolution `.cool` file | Input positions are 1-based; bin = `(position - 1) // bin_size` |

There is no separate `hicbam` mode. Both BAM modes accept Hi-C and long-read
alignments. Direct `paf2pairs` does not create intermediate concat PQS.
Converting via concat can give different positions from direct BAM/PAF-to-pairs;
see [import semantics](import.md) and [concat expansion](convert.md).

## Shared contracts

- Output paths must be new, and their parent directories must exist. For
  import/Cooler conversion, the `.partial` staging path must also be absent.
  Explicit scratch directories must already exist. Inputs remain unchanged.
- `min_mapq` defaults to 0. Order limits apply after alignment MAPQ filtering;
  the maximum is exclusive. Cooler conversion does not accept order limits.
- `threads` defaults to 1 and is a worker limit per stage, not a process-wide
  CPU cap. See each operation for queue and memory behavior.
- `chunk_size=1_000_000` and `batch_rows=65_536` are defaults, not strict RSS
  limits. Cooler uses the chunk size for sorting accepted pairs; PQS outputs
  use it for shard targets. Import grouping may additionally spill to scratch.
- Python operation reports provide `.to_dict()`; Rust conversion reports
  provide `.to_json()`; C delivers borrowed UTF-8 JSON through a callback.
  Report fields depend on the operation. Do not assume Cooler has PQS counts.
- New capabilities require corresponding native symbols. Updating the Python
  package alone does not update an older native library.

## Cooler-specific behavior

Python/CLI accept `10k`, `1m`, `1.5M` and integer bp; Rust/C accept integer bp.
Python/CLI require an explicit bin size, while `CoolOptions::default()` uses
10,000 bp. PQS input is read directly from q0 with five projected columns;
q1 is not added a second time. Output contains raw counts, without balancing
or multi-resolution `.mcool` generation.

Multiple workers can decode/bin PQS, sort contacts and compress pixel chunks.
All HDF5 calls remain on the caller. Small chunks/outputs and one-thread runs
retain ordinary HDF5 compression. See [resource use](cool.md#output-and-resource-use).
