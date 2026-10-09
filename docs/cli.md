# CLI

Complete [installation](installation.md), including adding the native executable
to PATH. Then run `pqsio` directly from your data directory.

```sh
pqsio --help
pqsio bam2pairs --help  # Options for a specific command
```

Replace the input filenames with your files. PQS paths are directories;
output paths must not already exist, and their parent directories must exist.

## Convert files

A typical BAM → PQS → Cooler workflow:

```sh
pqsio bam2pairs hic.bam -o hic.pairs.pqs --min-mapq 30 --threads 4
pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k --threads 4
```

Choose the command for your input and desired output:

| Command | Input → output |
| --- | --- |
| `bam2pairs` | Hi-C or long-read BAM → pairs PQS |
| `bam2concat` | BAM → concat PQS |
| `paf2pairs` | PAF or `.paf.gz` → pairs PQS |
| `paf2concat` | PAF or `.paf.gz` → concat PQS |
| `concat2pairs` | concat PQS → pairs PQS |
| `pairs2cool` | pairs PQS or pairs text → `.cool` |

All conversion commands take `INPUT -o OUTPUT`. Use `--min-mapq` to filter
alignments or pairs (default `0`) and `--threads` to set workers (default `1`).
`pairs2cool` also requires `--bin-size`, e.g. `10k` or `1m`, and writes raw counts
without balancing. Text `.pairs` → PQS import is not supported.

For paired-end Hi-C, use `bam2pairs` to keep cross-mate contacts;
`bam2concat` stores mates as separate reads. See [BAM/PAF import](import.md),
[concat-to-pairs](convert.md) and [Cooler output](cool.md) for detailed rules.
The equivalent `convert INPUT -o OUTPUT --mode MODE` syntax is also supported.

See the [concat workflow](concat-workflow.md) to retain all fragments of
selected reads and expand pairs only when needed.

## Inspect and export

```sh
pqsio info hic.pairs.pqs
pqsio head hic.pairs.pqs -n 10
pqsio stats hic.pairs.pqs --json
pqsio export hic.pairs.pqs -o hic.pairs.gz --threads 4
```

`info` summarizes the dataset, `head` previews rows, and `stats` scans quality
metrics. Export writes pairs or concat text; `.gz` enables compression.
Use `view` for a filtered preview or selected columns:

```sh
pqsio view hic.pairs.pqs --region chr1:0-1000000 --columns chrom1,pos1,chrom2,pos2
```

## Query, subset and merge

```sh
# Print contacts overlapping a region.
pqsio query hic.pairs.pqs --region chr1:0-1000000

# Save selected contacts as a new PQS dataset.
pqsio subset hic.pairs.pqs -o chr1.pairs.pqs --region chr1:0-1000000

# Merge datasets of the same format.
pqsio merge first.pairs.pqs second.pairs.pqs -o merged.pairs.pqs

# Check dataset structure and records.
pqsio validate hic.pairs.pqs --level full
```

Regions are **0-based, half-open**. Pairs match either endpoint by default.
Queries automatically build a missing index; use `--index off` to scan without
an index. Manage indexes explicitly with `index build`, `index status` and
`index rebuild`.

## Use in scripts

Conversions and file exports print JSON reports; `info` and `stats` use
`--json` for JSON output. Previews and queries print data to stdout. Progress
and errors go to stderr; add `--no-progress` to disable progress.

```sh
pqsio info hic.pairs.pqs --json > info.json
```

A zero exit code means success. Validation returns `1` for an invalid dataset
and `3` for an incomplete check; inspect its JSON report for details.

More options: `pqsio COMMAND --help`, or the reference pages for
[browse/export](browse.md), [statistics](stats.md), [queries](query.md),
[subset](subset.md), [merge](merge.md) and [validation](inspection.md).

## Native entry point

The CLI is a Rust executable using clap for parsing/help and native stderr
progress. Python/C APIs remain available independently. `-h`, `--help`, and
`-help` show help; `--bin-size`, `--binsize`, and `-bs` are equivalent.
`pip install .` supplies the Python API, not the CLI. See [installation](installation.md)
for binary paths and the optional `python -m pqsio` compatibility launcher.

See [native CLI performance](native-cli-performance.md) for a small, reproducible
startup/conversion/export/query benchmark and its measurement limits.
