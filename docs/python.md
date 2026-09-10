# Python API

Use `PYTHONPATH=pqsio/python` from the workspace root, or install this directory
as a Python package. The Python package does **not** compile or bundle the
native library; set `PQSIO_LIBRARY` to the absolute path of the built `.so`.
There is no Python runtime dependency beyond the standard library.

```python
from pqsio import Pair, Alignment, PairsWriter, ConcatWriter, Reader

with PairsWriter("sample.pairs.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_batch([Pair("read1", 0, 10, 0, 200, "+", "-", 60)])

with ConcatWriter("sample.concat.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_read([
        Alignment(1, 200, 0, 100, "+", 0, 10, 110, 60, 0.99),
        Alignment(1, 200, 100, 200, "-", 0, 500, 600, 20, 0.98),
    ])

with Reader("sample.concat.pqs", min_mapq=1) as reader:
    print(reader.kind, reader.contigs)
    for alignments in reader.iter_reads():
        print(alignments)
```

`PairsReader` and `ConcatReader` additionally check the dataset kind.
`iter_batches()` yields one Parquet shard at a time. Python objects are copies;
no borrowed native pointer escapes into the public API.

### Bulk concat writes (0.0.2)

```python
# Every inner list contains a complete read; read IDs increase across calls.
with ConcatWriter("bulk.concat.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_reads([
        [Alignment(1, 100, 0, 50, "+", 0, 10, 60, 30, 0.99)],
        [Alignment(2, 100, 0, 50, "-", 0, 100, 150, 60, 0.98)],
    ])
# Alternatively: writer.write_batch(flat_alignments, [0, end_read1, end_read2, ...])
```

The caller controls batch size; `write_reads` materializes its iterable.
All offsets and reads are validated before accepting a batch. Empty batches
use offsets `[0]`; empty reads are rejected. Input validation errors accept no
records from that batch; storage failures still poison/abort the staged output.

The new Python package remains usable with an old ABI v1 library for existing
methods; bulk writes report that a >=0.0.2 library is required. New C/C++ callers
use `pqsio_write_reads` / `Writer::write_reads(rows, offsets)`.

Bulk submission reduces native call overhead. It is not automatically faster
for Python dataclass inputs: fixed-field encoding already improves the existing
single-read method.
