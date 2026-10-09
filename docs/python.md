# Python API

Complete [installation](installation.md). Save examples in a `.py` file and
run `python example.py`, or `pixi run python example.py` in the source repository.
All output paths below must be new.

## Read and write pairs

```python
from pqsio import Pair, PairsWriter, Reader

with PairsWriter("sample.pairs.pqs", {"chr1": 1000}) as writer:
    writer.write_batch([Pair("read1", 0, 10, 0, 200, "+", "-", 60)])

with Reader("sample.pairs.pqs", min_mapq=30) as reader:
    for batch in reader.iter_batches():
        for pair in batch:
            print(pair.read_id, pair.pos1, pair.pos2)
```

Contig ID 0 means `chr1` here. Pairs positions are **1-based**.

## Read and write concat

```python
from pqsio import Alignment, ConcatWriter, Reader

with ConcatWriter("sample.concat.pqs", {"chr1": 1000}) as writer:
    writer.write_read([
        Alignment(1, 200, 0, 100, "+", 0, 10, 110, 60, 0.99),
        Alignment(1, 200, 100, 200, "-", 0, 500, 600, 20, 0.98),
    ])

with Reader("sample.concat.pqs") as reader:
    for alignments in reader.iter_reads():
        print(alignments)
```

Submit each complete read once, with strictly increasing `read_idx` values.
Concat intervals are **0-based, half-open**. See the [concat workflow](concat-workflow.md)
for full-read selection and deferred pair expansion.

## Convert

```python
import pqsio

pqsio.convert("hic.bam", "hic.pairs.pqs", mode="bam2pairs", min_mapq=30)
pqsio.convert("hic.pairs.pqs", "hic.10k.cool", mode="pairs2cool", bin_size="10k")
```

Other modes: `bam2concat`, `paf2pairs`, `paf2concat`, `concat2pairs`.
PAF imports require `contigsizes`. Always specify `mode` in scripts.

## Select and inspect

```python
import pqsio

print(pqsio.info("sample.pairs.pqs").to_dict())
pqsio.subset("sample.pairs.pqs", "selected.pairs.pqs",
             regions=[("chr1", 0, 500)], min_mapq=30)
pqsio.export("selected.pairs.pqs", "selected.pairs.gz")
print(pqsio.validate("sample.pairs.pqs", level="full").status)
```

Regions use 0-based, half-open coordinates. Validation reports can be `valid`,
`invalid` or `incomplete`; check the status.

## Choose compression

Writers default to Zstd. For example:

```python
from pqsio import PairsWriter

with PairsWriter("compressed.pairs.pqs", {"chr1": 1000},
                 compression="zstd", compression_level=6):
    pass
```

See [compression options](pqs-format.md#compression), [advanced I/O](reference.md#advanced-io)
and the [API reference](api.md) for remaining parameters.
