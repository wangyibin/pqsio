# pqsio

Read, write and convert genomic contacts with a Python API and a native Rust CLI.
PQS supports **selective column reads, fast quality filtering and efficient
compressed writing**. In our [one-million-record benchmark](pqs-format.md#performance-benchmark),
default PQS delivered **20.87–33.50× faster MAPQ ≥ 30 counts** and
**4.71–6.25× faster writes** than gzip text.

**Pairs PQS** stores pairwise contacts; **concat PQS** stores alignments grouped by read.
A `.pqs` dataset is a directory. See [PQS format](pqs-format.md) for its
layout, fields and coordinate conventions.

## 1. Install

For Conda users, see [Bioconda installation](installation.md#bioconda).
To build from source with [Pixi](https://pixi.sh), run on Linux:

```sh
git clone https://github.com/wangyibin/pqsio.git
cd pqsio
pixi install --locked
pixi run build
export PATH="$PWD/target/dev-release:$PATH"
```

Already have the source? Start from `cd pqsio`.
Pixi configures Python and the native library for you. The PATH setting
enables direct `pqsio` commands in the current shell.
See [installation](installation.md) for use in another Python environment.

## 2. Call the Python API

Save this as `example.py` and run `pixi run python example.py` from the repository.
It creates a tiny dataset, so no input files are needed.

```python
from pqsio import Pair, PairsWriter, Reader

with PairsWriter("sample.pairs.pqs", contigs={"chr1": 1000}) as writer:
    writer.write_batch([
        Pair(read_id="read1", chrom1=0, pos1=10, chrom2=0, pos2=200,
             strand1="+", strand2="-", mapq=60),
    ])

with Reader("sample.pairs.pqs") as reader:
    for batch in reader.iter_batches():
        print(batch)
```

`chrom1=0` and `chrom2=0` refer to the first contig (`chr1`); pair positions
are 1-based. The output directory must not already exist.
See the [Python API](python.md) for conversion, concat reads and filtering.

## 3. Use the CLI

Inspect and export the dataset created above:

```sh
pqsio info sample.pairs.pqs
pqsio head sample.pairs.pqs -n 10
pqsio export sample.pairs.pqs -o sample.pairs.gz
pqsio --help
```

See the [CLI guide](cli.md) for BAM/PAF conversion, Cooler output and region queries.
For other languages, see [Rust](rust.md) or [C/C++](native.md).
