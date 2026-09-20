# Python API

Complete [installation](installation.md), save examples in a `.py` file, then
run `pixi run python your_script.py` from the repository. If installed in your
own Python environment, use `python your_script.py`.

## Convert files

Use `pqsio.convert(input, output, mode=...)`. Replace the input filenames with
your own files; outputs must be new paths with existing parent directories.

```python
import pqsio

# Hi-C or long-read BAM → pairs PQS
result = pqsio.convert("hic.bam", "hic.pairs.pqs", mode="bam2pairs",
                       min_mapq=30, threads=4)
print(result.to_dict())

# Pairs PQS → Cooler contact matrix, with 10 kb bins
pqsio.convert("hic.pairs.pqs", "hic.10k.cool", mode="pairs2cool",
              bin_size="10k", threads=4)
```

| `mode` | Input → output |
| --- | --- |
| `bam2pairs` | BAM → pairs PQS |
| `bam2concat` | BAM → concat PQS |
| `paf2pairs` | PAF or `.paf.gz` → pairs PQS |
| `paf2concat` | PAF or `.paf.gz` → concat PQS |
| `concat2pairs` | concat PQS → pairs PQS |
| `pairs2cool` | pairs PQS or pairs text → `.cool`; requires `bin_size` |

Always specify `mode` in your scripts; the default is `concat2pairs`, regardless
of the filename. Common options are `min_mapq` (default `0`) and `threads`
(default `1`). Cooler output contains raw counts, without balancing.
See [conversion details](api.md#conversion-modes-and-coordinates) for coordinates
and mode-specific rules.

## Read and write pairs

This example needs no input files. Run it with a new output path each time.

```python
from pqsio import Pair, PairsWriter, Reader

with PairsWriter("sample.pairs.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_batch([
        Pair(read_id="read1", chrom1=0, pos1=10, chrom2=0, pos2=200,
             strand1="+", strand2="-", mapq=60),
    ])

with Reader("sample.pairs.pqs", min_mapq=30) as reader:
    print(reader.kind, reader.contigs)
    for batch in reader.iter_batches():
        for pair in batch:
            print(pair.read_id, pair.pos1, pair.pos2)
```

Contig IDs are zero-based indexes into `contigs`: `0` means `chr1` here.
Pair positions are **1-based**. The writer finishes the dataset when its `with`
block exits successfully. `Reader.iter_batches()` returns one shard at a time;
use [StreamingReader](streaming.md) when you need smaller batches.

## Read and write concat

Write all alignments of a read together. Each new read needs a strictly
increasing integer `read_idx`; alignment intervals are **0-based, half-open**.

```python
from pqsio import Alignment, ConcatWriter, Reader

with ConcatWriter("sample.concat.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_read([
        Alignment(read_idx=1, read_length=200, read_start=0, read_end=100,
                  strand="+", chrom=0, start=10, end=110,
                  mapping_quality=60, identity=0.99),
        Alignment(read_idx=1, read_length=200, read_start=100, read_end=200,
                  strand="-", chrom=0, start=500, end=600,
                  mapping_quality=20, identity=0.98),
    ])

with Reader("sample.concat.pqs") as reader:
    for alignments in reader.iter_reads():
        print(alignments)
```

For bulk writes, `writer.write_reads([read1_alignments, read2_alignments])`
accepts a list of complete reads. See [columnar I/O](columnar.md) and
[parallel writing](parallel.md) for larger workloads.

## Choose compression

`PairsWriter`, `ConcatWriter` and `ParallelWriter` accept `compression` and
`compression_level`. The default remains Zstd with its codec default level.
Both q0 and q1 use the selected setting; readers detect the codec automatically.

```python
from pqsio import Pair, PairsWriter

with PairsWriter("compressed.pairs.pqs", {"chr1": 1000},
                 compression="zstd", compression_level=6) as writer:
    writer.write_batch([Pair("read1", 0, 10, 0, 200, "+", "-", 60)])

with PairsWriter("uncompressed.pairs.pqs", {"chr1": 1000},
                 compression="uncompressed") as writer:
    writer.write_batch([Pair("read1", 0, 10, 0, 200, "+", "-", 60)])
```

See [codecs and level ranges](pqs-format.md#compression). Omit
`compression_level` for the codec default; `0` is an explicit level for gzip
and Brotli. Invalid settings raise `ValueError` before creating output.
These options apply to writer constructors; conversion, subset, merge and CLI
commands continue to use their existing defaults.

## Inspect, filter and export

These calls use the pairs dataset created above:

```python
import pqsio

print(pqsio.info("sample.pairs.pqs").to_dict())
print(pqsio.stats("sample.pairs.pqs").to_dict())

pqsio.subset("sample.pairs.pqs", "selected.pairs.pqs",
             regions=[("chr1", 0, 500)], min_mapq=30)
pqsio.export("selected.pairs.pqs", "selected.pairs.gz")

report = pqsio.validate("sample.pairs.pqs", level="full")
print(report.status)  # Check for valid, invalid or incomplete.
```

Regions are **0-based, half-open**, including for pairs. Operation reports
provide `.to_dict()`. Validation can report `invalid` or `incomplete` without
raising an exception, so check its status.

More operations: [queries and indexes](query.md), [subset](subset.md),
[merge](merge.md), [export](browse.md) and [copy numbers](copy-numbers.md).
For full conversion contracts and other languages, see the [API reference](api.md).
