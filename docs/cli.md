# Command-line interface

The Python CLI dispatches all conversion record processing and PQS operations
to Rust; no alignment or pair records pass through Python.
Help and usage errors use rich-click, following CPhasing's command/option
groups and terminal styling. `-h`, `--help` and `-help` are equivalent on
the root command and every subcommand. Running `pqsio` without arguments
prints help and exits successfully.
From the repository, build the library once and use the Pixi task:

```sh
pixi run build
pixi run pqsio --help
pixi run pqsio convert sample.concat.pqs -o sample.pairs.pqs
```

To install the `pqsio` command into a Python environment (Python >= 3.9 with pip),
run the following from the repository:

```sh
python -m pip install .
export PQSIO_LIBRARY="$PWD/target/dev-release/libpqsio.so"
pqsio --help
pqsio --version
```

Installation includes rich-click and its dependencies. The Python package
requires a separately built native library; installation does
not compile or bundle it. `PQSIO_LIBRARY` must point to that shared library unless
it is discoverable by the system loader. `python -m pqsio` is equivalent to the
installed command. Help and version work without a native library.

## Commands

Conversion, inspection, validation, subset, merge, index operations and file export print JSON
reports to stdout. `info` and `stats` print tables (`--json` for JSON); `head`, `view`
and `export -o -` print only data. `query` also prints data by default, or a JSON
report when writing to a file. Errors and progress go to stderr. Use
`pqsio COMMAND --help` for the complete option list. Dataset paths are PQS
directories; conversion also supports input files and `.cool` output.
Output paths must not already exist, and their parents must exist.
Help is printed to stdout; tables, help, diagnostics and progress use Rich.
Successful reports remain plain JSON, including when redirected to a file.
Terminal color detection is automatic; set `NO_COLOR=1` to disable colors.

| Command | Syntax | Purpose |
| --- | --- | --- |
| `info` | `pqsio info PATH [--json] [--stats]` | Metadata/footer summary; optional MAPQ scan |
| `stats` | `pqsio stats PATH [--min-mapq 30] [--json]` | One-pass pairs/concat quality summary |
| `query` | `pqsio query INPUT --region CHROM:START-END` | Region union to stdout or a text file |
| `index` | `pqsio index build\|status\|rebuild PATH` | Independent q0/q1 region-index management |
| `head` | `pqsio head INPUT [-n 10]` | First matching rows as TSV |
| `view` | `pqsio view INPUT [-n 100 \| --all]` | TSV preview or filtered stream |
| `export` | `pqsio export INPUT -o OUTPUT` | Native pairs/concat/TSV export, gzip/mgzip by suffix |
| `convert` | `pqsio convert INPUT -o OUTPUT --mode MODE` | Six native conversion modes below |
| `inspect` | `pqsio inspect PATH` | PQS metadata and Parquet footer report |
| `validate` | `pqsio validate PATH [--level quick\|full]` | PQS diagnostic report |
| `subset` | `pqsio subset INPUT -o OUTPUT [filters]` | Select records, preserving PQS format |
| `merge` | `pqsio merge INPUT... -o OUTPUT` | Merge same-format PQS inputs in given order |

All six modes also have direct commands: `bam2pairs`, `bam2concat`, `paf2pairs`,
`paf2concat`, `concat2pairs` and `pairs2cool`. For example,
`pqsio pairs2cool INPUT -o OUTPUT --bin-size 10k` is equivalent to
`pqsio convert INPUT -o OUTPUT --mode pairs2cool --bin-size 10k`.
Direct commands expose only the options relevant to their mode and reuse the
same Rust implementation. `convert --mode` remains supported. Both BAM commands
accept Hi-C and long-read BAM; no separate `hicbam` command is needed.
Copy-number operations remain API-only.

```sh
# Convert concat PQS to pairs PQS; max-order is exclusive, after MAPQ filtering.
pqsio convert sample.concat.pqs -o sample.pairs.pqs \
    --mode concat2pairs --min-mapq 1 --min-order 2 --max-order 10 --threads 4

# Both BAM output modes accept Hi-C and long-read BAM.
pqsio convert hic.bam --mode bam2pairs -o hic.pairs.pqs
pqsio convert long_reads.bam --mode bam2concat -o long_reads.concat.pqs
pqsio convert reads.paf.gz --mode paf2concat -o reads.concat.pqs
pqsio convert reads.paf.gz --mode paf2pairs -o reads.pairs.pqs

# Bin pairs PQS or pairs text directly into a single-resolution Cooler file.
pqsio convert hic.pairs.pqs --mode pairs2cool --bin-size 10k --threads 4 -o hic.10k.cool
pqsio convert hic.pairs.gz --mode pairs2cool --bin-size 1m \
    --contigsizes genome.chrom.sizes -o text.1m.cool

# Inspect metadata and Parquet footers without decoding records.
pqsio inspect sample.pqs

# Validate records as well as structure; default level is quick.
pqsio validate sample.pqs --level full --max-issues 100

# Regions are 0-based half-open; repeat --region or --chrom to select several.
pqsio subset sample.pairs.pqs -o selected.pairs.pqs \
    --region chr1:100000-200000 --min-mapq 30 --pairs-mode either

# Concat logical IDs use --read-index; pairs string IDs use --read-id.
pqsio subset sample.concat.pqs -o selected.concat.pqs \
    --read-index 7 --read-index 9 --mode complete-reads

# Inputs must have the same format; their given order is preserved.
pqsio merge first.pqs second.pqs -o merged.pqs
```

`convert` defaults to `concat2pairs` (concat PQS to pairs PQS), with
the [same coordinate, ID and filtering rules as the API](convert.md). It also
supports `bam2pairs`, `bam2concat`, `paf2pairs`, `paf2concat` and `pairs2cool`; see
[BAM/PAF import](import.md) for grouping, flags and coordinate rules.
[Pairs to Cooler](cool.md) accepts pairs PQS or text, requires `--bin-size`, and
produces raw contact counts without balancing. Text `.pairs` to PQS import is
not supported. Default concat-to-pairs filters are
`--min-mapq 0`, `--min-order 2` and no upper order limit; threads default to 1.
For `concat2pairs`, `--threads` controls the conversion and encoding workers,
not a total CPU cap. For imports it controls BAM/mgzip decoding and native Parquet encoding;
PAF parsing and pair expansion run in Rust on the calling thread. BAM/PAF concat imports default to minimum order 1, retaining singletons.
For `pairs2cool`, threads control mgzip decoding, PQS row-group decoding/binning
and sorting/pixel-compression workers per stage. Final merging and all HDF5
calls remain on one thread; compression workers supply standard shuffle/gzip
chunks for direct writing. One thread and small chunks retain ordinary HDF5
compression; see [pairs2cool](cool.md) for limits and memory details.
`--chunk-size` bounds accepted records per sorting run; inputs fitting one run
stay in memory without scratch runs. `--batch-rows` controls PQS pixel batches
and HDF5 write buffers, not the decoded Parquet row-group size. Pipeline
queues are bounded, but increasing threads can increase memory use.

`convert`, `subset` and `merge` accept `--batch-rows` (default 65536) and
`--chunk-size` (default 1000000). These are targets, not strict memory limits.
`subset` and `merge` write provenance by default; use `--no-provenance` to omit it.
Subset retains its input format. Its concat modes are `matching-alignments`
(default) and `complete-reads`; pairs modes are `either` (default) and `both`.
Pairs `--read-id` preserves strings, including leading zeros; concat
`--read-index` accepts unsigned integer logical IDs. The two options are mutually
exclusive. See [subset](subset.md), [merge](merge.md) and
[inspection/validation](inspection.md) for the underlying storage contracts.

## Conversion options

The default mode is `concat2pairs`; file extensions do not select a mode.

| `--mode` | Input | Output |
| --- | --- | --- |
| `concat2pairs` | concat PQS directory | pairs PQS directory |
| `bam2pairs` | Hi-C or long-read BAM | pairs PQS directory |
| `bam2concat` | Hi-C or long-read BAM | concat PQS directory |
| `paf2pairs` | PAF, optionally gzip/mgzip | pairs PQS directory, directly |
| `paf2concat` | PAF, optionally gzip/mgzip | concat PQS directory |
| `pairs2cool` | pairs PQS directory or pairs text, optionally gzip/mgzip | single-resolution `.cool` file |

| Option | Default | Meaning and supported modes |
| --- | --- | --- |
| `-o`, `--output` | Required | New output directory/file |
| `--min-mapq` | `0` | Integer 0–255; filter alignments or stored pair MAPQ |
| `--min-order` | 2 for pairs output; 1 for concat imports | Minimum retained alignment count; not for `pairs2cool` |
| `--max-order` | Unlimited | Exclusive upper order bound, greater than minimum; not for `pairs2cool` |
| `-t`, `--threads` | `1` | Positive native worker limit per stage |
| `--chunk-size` | `1000000` | PQS shard target or Cooler accepted-record sort-run limit |
| `--batch-rows` | `65536` | Positive transfer/buffer target |
| `--contigsizes` | Input-derived dictionary | Optional sizes/FAI for PAF or pairs text; not BAM or pairs PQS |
| `--include-secondary` | Off | Include secondary BAM/PAF alignments |
| `--pair-position` | `leftmost` | `leftmost` or `five-prime`, only `bam2pairs`/`paf2pairs` |
| `--tmpdir` | Output parent | Existing scratch directory for BAM/PAF imports or Cooler |
| `--bin-size`, `--binsize`, `-bs` | Required for `pairs2cool` | Integer bp or case-insensitive decimal `k`/`m`/`g` units; only Cooler |

Bin-size examples: `10k` = 10,000 bp, `1m` = 1,000,000 bp, `1.5M` =
1,500,000 bp. Fractional bp after scaling are truncated; the result must be
positive and fit Int64. `--chunk-size` and `--batch-rows` accept integer row
counts, not these suffixes. The obsolete hidden `--samtools` option is rejected;
BAM reading uses the native library directly.

The pair-expansion modes filter MAPQ before order selection. `bam2pairs` and
`paf2pairs` default to leftmost positions, whereas `concat2pairs` uses reference
midpoints. For paired BAM, use `bam2pairs` directly when cross-mate contacts
are needed; `bam2concat` stores mates as separate logical reads. See
[coordinates and grouping](import.md).

For Cooler, `--threads 4` enables parallel PQS decoding/binning, sorting and
pixel compression where applicable. One thread performs all HDF5 calls.
Larger sorting chunks can reduce merge passes while increasing memory use:

```sh
pqsio convert sample.pairs.pqs --mode pairs2cool --bin-size 20k \
    --min-mapq 1 --threads 4 --chunk-size 4000000 -o sample.20k.cool
```

This produces raw contact counts, not a balanced matrix or a multi-resolution
file. See [Cooler input/resource rules](cool.md).

## Other command options

| Command | Options and defaults |
| --- | --- |
| `inspect` | Only the PQS directory path |
| `validate` | `--level quick` (or `full`); `--max-issues 100`, positive |
| `subset` | Optional `--min-mapq`; repeated `--chrom CONTIG`, `--region CHROM:START-END`, `--read-id ID` or `--read-index ID` |
| `subset` modes | Pairs: `--pairs-mode either` (or `both`); concat: `--mode matching-alignments` (or `complete-reads`) |
| `subset`, `merge` output | Required `-o`; `--chunk-size 1000000`, `--batch-rows 65536`; provenance on unless `--no-provenance` |

`--read-id` and `--read-index` are mutually exclusive; use strings for pairs
and unsigned integers for concat. Regions require `0 <= START < END`.
CLI concat mode names use hyphens; Python uses `matching_alignments` and
`complete_reads`. `--threads` belongs to `convert`, not `subset` or `merge`.
`inspect` and `validate` operate on PQS directories, not `.cool` files.

## Reports and scripting

Capture stdout for JSON and stderr separately for diagnostics:

```sh
pqsio convert sample.pairs.pqs --mode pairs2cool --bin-size 10k \
    -o sample.10k.cool > conversion.json
```

All conversion reports include `output`, `mode` and `format`. PQS conversion
reports include `counts.q0_records` and `counts.q1_records`; Cooler reports
instead include `bin_size`, `input_records`, `skipped_mapq`, `skipped_unmapped`,
`nchroms`, `nbins`, `nnz` and `sum`. `nnz` counts occupied upper-triangle pixels;
`sum` counts accepted contacts, including duplicates and diagonal contacts.
Python provides the same reports via `.to_dict()`; see the [API overview](api.md).

## Exit status

| Code | Meaning |
| --- | --- |
| 0 | Success; validation status is `valid` |
| 1 | Operation failed, or validation status is `invalid` |
| 2 | Argument errors or an operation raising `ValueError`, including invalid BAM/PAF/Cooler input |
| 3 | Validation status is `incomplete` |
| 130 | Interrupted with Ctrl-C |

Validation reports are printed even for `invalid` and `incomplete` results.
Check both the exit code and the JSON report when using the CLI in a pipeline.

## Browse, export and progress

```sh
pqsio info sample.pairs.pqs --stats
pqsio head sample.pairs.pqs -n 10
pqsio view sample.pairs.pqs --region chr1:0-1000000 --columns chrom1,pos1,chrom2,pos2
pqsio export sample.pairs.pqs -o sample.pairs.gz --threads 4
```

`head` defaults to 10 rows, `view` to 100; use `view --all` for a full stream.
Both emit TSV. Export defaults to pairs/concat text according to input kind when exporting
all columns, otherwise TSV. Use `--format concat` explicitly for headerless
11-column concat text, or `--format tsv` for a TSV header. Export to `-o -` streams data to stdout without a report.
All commands accept `--progress/--no-progress`; the default enables progress
only when stderr is a terminal. See [browse and export](browse.md) for all
columns, filtering, output formats and progress semantics.

## Statistics, queries and indexes

```sh
pqsio stats sample.pairs.pqs --json
pqsio index build sample.pairs.pqs --quality both
pqsio index status sample.pairs.pqs
pqsio index rebuild sample.pairs.pqs --quality q1
pqsio query sample.pairs.pqs --region chr1:0-1000000 --min-mapq 30 \
    --index require --show-stats
```

See [quality statistics](stats.md) for pairs/concat metrics and [region queries](query.md)
for query output modes, complete-read selection, index status and lifecycle.

By default, `query --index auto` builds a missing index for its selected q0/q1
partition before querying. Use `--no-build-index` for read-only auto fallback
or `--index off` to scan directly. Existing invalid indexes require an explicit
`index rebuild`; complete-read queries do not auto-build an unused index.
