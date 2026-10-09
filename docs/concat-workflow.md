# Complete-read concat workflow

Select a read by one matching alignment, keep all its stored fragments, and
expand pairs only when needed. Selection MAPQ and pair-expansion MAPQ are
independent.

## Use your data

```sh
pqsio subset sample.concat.pqs -o selected.concat.pqs \
  --region chr1:100000-200000 --min-mapq 30 --mode complete-reads
pqsio concat2pairs selected.concat.pqs -o mapq30.pairs.pqs --min-mapq 30
```

The first command retains every stored alignment of reads with an alignment
meeting both the region and MAPQ conditions, including distant/low-quality
fragments. The second pairs fragments passing its own threshold.
Skip the second command to defer expansion, or reuse selected concat later:

```sh
pqsio concat2pairs selected.concat.pqs -o mapq0.pairs.pqs --min-mapq 0 --max-order 20
```

## Try a small demo

From the source repository, with a new output directory:

```sh
pixi run python examples/concat_workflow.py --demo --output concat-demo --expand
```

No download is needed. The demo selects one read with three fragments,
including a distant MAPQ 0 fragment. It saves:

| Output | Contents |
| --- | --- |
| `selected.concat.pqs` | The complete selected read, all three fragments |
| `pairs.pqs` | One pair at the default expansion MAPQ 30 |
| `workflow.json` | Selection/expansion counts and thresholds |

Expanding the saved concat at MAPQ 0 gives three pairs. The selected concat
remains unchanged. The script also accepts `--input`, repeatable `--region`
and `--read-index`, separate `--select-mapq`/`--pair-mapq`, and order limits.
Run `pixi run python examples/concat_workflow.py --help` for options.

## Keep these rules in mind

- Regions/concat intervals are 0-based, half-open; pair positions are 1-based
  reference midpoints. Resulting pairs need not overlap the selection region.
- Order limits apply after expansion MAPQ filtering; `max_order` is exclusive.
  A read with `n` eligible fragments creates `n*(n-1)/2` pairs.
- Import with MAPQ 0/order 1 to retain those records; selection cannot restore
  alignments discarded during import. Paired BAM mates are separate concat reads.
- Complete-read selection scans q0. Each output is published independently;
  completed concat remains usable if pair expansion fails.

See [subset](subset.md), [conversion](convert.md) and [import](import.md) for
ID, metadata and filtering contracts, and [compatibility](storage.md) for
external-reader limits.
