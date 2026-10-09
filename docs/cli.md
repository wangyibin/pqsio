# CLI

After [installation](installation.md), run `pqsio` from your data directory.
Replace input filenames below with your own; output paths must be new and
have existing parent directories.

## Convert

```sh
pqsio bam2pairs hic.bam -o hic.pairs.pqs --min-mapq 30 --threads 4
pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k --threads 4
```

| Command | Input → output |
| --- | --- |
| `bam2pairs`, `bam2concat` | BAM → pairs or concat PQS |
| `paf2pairs`, `paf2concat` | PAF/PAF.gz → pairs or concat PQS; requires `--contigsizes` |
| `concat2pairs` | concat PQS → pairs PQS |
| `pairs2cool` | pairs PQS/text → Cooler; requires `--bin-size` |

`--min-mapq` defaults to 0; `--threads` defaults to 1. Cooler contains raw counts.
For paired-end Hi-C use `bam2pairs`: `bam2concat` stores mates as separate reads.
See the [concat workflow](concat-workflow.md) to preserve multi-fragment reads.

## Inspect and export

```sh
pqsio info hic.pairs.pqs
pqsio head hic.pairs.pqs -n 10
pqsio stats hic.pairs.pqs --json
pqsio export hic.pairs.pqs -o hic.pairs.gz
pqsio validate hic.pairs.pqs --level full
```

## Select and combine

```sh
pqsio query hic.pairs.pqs --region chr1:0-1000000
pqsio subset hic.pairs.pqs -o selected.pairs.pqs --region chr1:0-1000000
pqsio merge first.pairs.pqs second.pairs.pqs -o merged.pairs.pqs
```

Regions are **0-based, half-open**. Pairs match either endpoint by default.
Queries can build missing indexes; use `--index off` for a sequential scan.

## Help and scripting

```sh
pqsio --help
pqsio concat2pairs --help
pqsio info hic.pairs.pqs --json > info.json
```

Conversion and export reports are JSON. Progress/errors go to stderr;
`--no-progress` disables progress. Validation exits 1 for invalid data and
3 for incomplete checks. Other failures use nonzero exit codes.
See the [reference](reference.md) for filtering, indexes and conversion rules.
