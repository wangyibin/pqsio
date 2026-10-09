# Select complete reads, expand pairs when needed

Keep concat PQS as the reusable alignment dataset. Select a read when one
alignment overlaps a region and passes a quality threshold, retain **all** its
stored alignments, and choose the pair-expansion threshold separately.
This preserves distant and low-quality fragments for later analysis.

## Run a small example

From the repository after [installation](installation.md):

```sh
pixi run build
pixi run python examples/concat_workflow.py --demo --output concat-demo --expand
```

`concat-demo` must be a new path with an existing parent. No download is needed.
The example creates three reads and selects on `chr1:100-200` with MAPQ >= 30:

| Read | Stored alignments | Selection result |
| --- | --- | --- |
| 7 | chr1 anchor (MAPQ 60), distant chr2 fragment (40), distant chr1 fragment (0) | All three alignments retained |
| 9 | chr1 anchor (10), distant chr2 fragment (60) | Rejected: no single alignment meets both region and quality conditions |
| 11 | Two high-quality alignments outside the region | Rejected |

The output contains:

- `input.concat.pqs`: synthetic input, only when `--demo` is used.
- `selected.concat.pqs`: one complete read, three alignments, including MAPQ 0.
- `pairs.pqs`: one pair after independently requiring alignment MAPQ >= 30.
- `workflow.json`: selection/expansion counts and thresholds; expansion is null
  if `--expand` is omitted. Selection provenance lives in the selected dataset.

The pair is `7:0:1`, from chr1 position 126 to chr2 position 326, MAPQ 40.
Pair coordinates are 1-based reference midpoints. Alignment and region
intervals are 0-based, half-open. The original stored alignment order and
fields remain intact in `selected.concat.pqs`.

## Use your own concat data

```sh
pixi run python examples/concat_workflow.py \
  --input sample.concat.pqs --output region-analysis \
  --region chr1:100000-200000 --select-mapq 30
```

This only extracts complete reads; it creates no pairs. Repeat `--region` to
select the union of regions, or repeat `--read-index` to restrict logical read
IDs. Region, ID and quality conditions intersect. With real input there is no
implicit region: omitting regions selects reads by ID/MAPQ alone.

Expand immediately if desired:

```sh
pixi run python examples/concat_workflow.py \
  --input sample.concat.pqs --output region-with-pairs \
  --region chr1:100000-200000 --select-mapq 30 \
  --expand --pair-mapq 20 --min-order 2 --max-order 20 --threads 4
```

Or reuse the saved concat later, with different thresholds:

```sh
pqsio concat2pairs region-analysis/selected.concat.pqs \
  -o mapq30.pairs.pqs --min-mapq 30
pqsio concat2pairs region-analysis/selected.concat.pqs \
  -o mapq0.pairs.pqs --min-mapq 0 --max-order 20
```

For the demo, these two thresholds produce one and three pairs respectively.
No original BAM/PAF import is repeated. Selected concat remains unchanged.

## Equivalent native CLI and Python API

The example composes existing native operations; no new storage format or
runtime dependency is required. The same workflow is:

```sh
pqsio subset sample.concat.pqs -o selected.concat.pqs \
  --region chr1:100000-200000 --min-mapq 30 --mode complete-reads
pqsio concat2pairs selected.concat.pqs -o selected.pairs.pqs \
  --min-mapq 20 --min-order 2 --max-order 20
```

```python
import pqsio

pqsio.subset("sample.concat.pqs", "selected.concat.pqs",
             regions=[("chr1", 100_000, 200_000)], min_mapq=30,
             mode="complete_reads")
pqsio.convert("selected.concat.pqs", "selected.pairs.pqs",
              mode="concat2pairs", min_mapq=20, min_order=2, max_order=20)
```

For a fresh BAM/PAF import, keep `min_mapq=0` and `min_order=1` if low-quality
and single-alignment reads must remain available. Review [import rules](import.md):
secondary alignments are excluded by default and paired BAM mates become
separate concat reads. Selection cannot restore records discarded on import.

## Interpretation and limits

- **Selection quality and expansion quality are different decisions.** The
  first identifies reads; the second removes alignments only from the derived
  pair output. Reading selected concat with a positive MAPQ filter would hide
  some retained alignments; use MAPQ 0 to inspect the complete stored read.
- All eligible fragments of selected reads are paired, including distant
  fragments and same-contig contacts. Pairs need not overlap the anchor region.
- Order means retained alignment count **after pair-MAPQ filtering**, not
  distinct chromosome/locus count. `min_order` is inclusive, `max_order`
  exclusive. A read with `n` eligible alignments generates `n*(n-1)/2` pairs.
  Expansion is buffered in batches but its output size is still quadratic.
- Pair IDs contain the logical read ID and sorted indices **after filtering**.
  Those indices can change with thresholds; they are not persistent original
  fragment identifiers. See [conversion semantics](convert.md).
- Complete-read subset scans q0 sequentially; no read-locator index or random
  access is added. Memory includes decoded row groups and the largest read.
- Preservation covers PQS alignment fields, logical IDs, the contig table and
  explicit `cn.info`. It does not preserve arbitrary BAM tags, sequence/quality
  strings, original QNAMEs or unrelated sidecars. See [subset contracts](subset.md).
- Each dataset is published independently. If expansion fails, completed
  selected concat remains usable; the workflow is not a multi-output transaction.
  Existing workflow output directories are rejected, not overwritten.
- Use native pqsio readers for this workflow. CPhasing with PyArrow 10.0.1 cannot
  read generated concat; see the [compatibility boundary](storage.md).
