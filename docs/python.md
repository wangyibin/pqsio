# Python API

Use `PYTHONPATH=pqsio/python` from the workspace root, or install this directory
as a Python package. The Python package does **not** compile or bundle the
native library; set `PQSIO_LIBRARY` to the absolute path of the built `.so`.
The storage bindings use the standard library. Installing the package also
installs `rich-click` and its dependencies for the command-line interface.

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

## Dataset operations

The [API overview](api.md) maps Python calls to Rust and C entry points.
The public conversion signature is:

```python
convert(input, output, mode='concat2pairs', *, chunk_size=1_000_000,
        batch_rows=65_536, min_mapq=0, min_order=None, max_order=None,
        threads=1, contigsizes=None, include_secondary=False, samtools=None,
        tmpdir=None, pair_position=None, bin_size=None)
```

Paths accept strings or path-like objects. Results are `ConvertResult` objects;
call `.to_dict()` for an independent dictionary copy. `samtools` is an obsolete
compatibility parameter: leave it as `None`; supplying it to a BAM import fails.

```python
import pqsio

# Direct BAM/PAF import; choose the desired output format.
report = pqsio.convert("hic.bam", "hic.pairs.pqs", mode="bam2pairs",
                       min_mapq=1, threads=4).to_dict()
report = pqsio.convert("reads.paf.gz", "reads.concat.pqs", mode="paf2concat",
                       threads=4).to_dict()

# Direct PQS read and native Cooler output: no Python record iteration.
report = pqsio.convert("hic.pairs.pqs", "hic.10k.cool", mode="pairs2cool",
                       bin_size="10k", min_mapq=1, threads=4,
                       chunk_size=4_000_000).to_dict()
print(report["nbins"], report["nnz"], report["sum"])
```

| Keyword | Default | Applicability |
| --- | --- | --- |
| `mode` | `concat2pairs` | Six modes listed in the [API overview](api.md) |
| `min_mapq` | `0` | Integer 0–255; alignment filter for imports/concat, pair filter for Cooler |
| `min_order` | `None` | Resolves to 2 for pairs output, 1 for concat imports |
| `max_order` | `None` | Exclusive upper order limit; `None` means unlimited |
| `threads` | `1` | Positive worker limit per stage |
| `chunk_size`, `batch_rows` | `1_000_000`, `65_536` | Positive buffering/shard/sort targets; see operation-specific memory rules |
| `contigsizes` | `None` | Optional sizes/FAI for PAF or pairs text; BAM/PQS use their dictionaries |
| `include_secondary` | `False` | BAM/PAF imports only |
| `pair_position` | `None` | `bam2pairs`/`paf2pairs`: resolves to `leftmost`; alternative `five-prime` |
| `tmpdir` | `None` | BAM/PAF and Cooler scratch; defaults to output parent |
| `bin_size` | `None` | Required only for `pairs2cool`; integer bp or unit string |

`pairs2cool` rejects order, secondary-alignment and pair-position options.
Python numeric `bin_size` must be an integer (not a float or bool); use a
string such as `"1.5m"` for fractional units. See [Cooler rules](cool.md) and
[BAM/PAF rules](import.md) before choosing a conversion route.

Other high-level functions return report objects with `.to_dict()`:

```python
inspect(path)
validate(path, level='quick', *, max_issues=100)
subset(input, output, *, min_mapq=None, chroms=None, regions=None,
       read_ids=None, pairs_mode=None, mode=None, batch_rows=65_536,
       chunk_size=1_000_000, provenance=True)
merge(inputs, output, *, chunk_size=1_000_000, batch_rows=65_536,
      provenance=True)
```

Example calls:

```python
overview = pqsio.inspect("hic.pairs.pqs")
validation = pqsio.validate("hic.pairs.pqs", level="full", max_issues=100)
print(validation.status)  # valid, invalid, or incomplete; inspect this explicitly
selected = pqsio.subset("hic.pairs.pqs", "selected.pqs",
                         regions=[("chr1", 0, 100_000)], min_mapq=1)
merged = pqsio.merge(["first.pqs", "second.pqs"], "merged.pqs")
```

Region coordinates are 0-based half-open. See [subset](subset.md),
[merge](merge.md), and [validation](inspection.md) for complete signatures and
selection/report semantics. `inspect` and `validate` accept PQS directories,
not `.cool` files.

Invalid Python arguments raise `ValueError`; native operation failures may
raise `RuntimeError`, and library-loading failures may raise `OSError`.
BAM/PAF and Cooler bindings also map native invalid-input errors to `ValueError`.
A validation report can have `invalid` or `incomplete` status without raising.

## Bulk concat writes (0.0.2)

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

## Summaries and text export

`pqsio.info(path, stats=False)` returns a metadata/footer summary; `stats=True`
scans q0 for MAPQ and concat read counts. `pqsio.view(path, limit=100)` previews
TSV on OS stdout; `pqsio.export(path, output, threads=4)` writes pairs/concat/TSV,
compressed for `.gz`/`.mgz`. Record handling stays in Rust. Each returns a report
with `.to_dict()`. See [browse and export](browse.md) for complete signatures,
column names, filters and stdout behavior.

`pqsio.stats(path, min_mapq=0).to_dict()` provides a one-pass q0 quality summary
([metrics](stats.md)). `pqsio.index_status(path, quality="q0").to_dict()` reports
index availability ([index management](query.md)). For native text queries,
`export`/`view` additionally accept `index="auto"` and `filter_mode=None`;
`filter_mode="complete_reads"` selects complete concat reads matching a region.
