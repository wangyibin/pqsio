# pqsio

Read, write and convert genomic contacts with a **Python API** and **native Rust CLI**. Supports pairs PQS, alignment-level concat PQS, BAM/PAF import and
Cooler output. Independent of CPhasing's phasing and alignment algorithms.

## Release scope

The current source release targets Linux x86-64 with the locked Pixi
environment and native pqsio readers/writers. CPhasing with PyArrow 10.0.1
cannot read generated concat PQS; that integration is **not supported**.
No minimum compatible PyArrow version is claimed. aarch64 and prebuilt
Python/Conda packages are outside the validated release scope. See
[compatibility](docs/storage.md) and [release validation](docs/development.md).

## Install

### Bioconda

```sh
conda create -n pqsio --override-channels -c conda-forge -c bioconda \
  --strict-channel-priority pqsio
conda activate pqsio
pqsio --help
```

See [installation](docs/installation.md#bioconda) for channel configuration.

### Build from source with Pixi

With [Pixi](https://pixi.sh) installed, run on Linux:

```sh
git clone https://github.com/wangyibin/pqsio.git
cd pqsio
pixi install --locked
pixi run build
export PATH="$PWD/target/dev-release:$PATH"
```

Already have the source? Run the install, build and PATH commands from its
`pqsio` directory. The PATH setting enables direct `pqsio` commands in the
current shell.
Pixi configures Python and the native library automatically. To install into
another Python environment, see [installation](docs/installation.md).

## Python API

Save as `example.py`, then run `pixi run python example.py`:

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

Contig ID `0` means `chr1` here; pair positions are 1-based. A `.pqs` dataset
is a directory, and the output path must be new.

## CLI

Inspect the dataset created above, or convert your own BAM file:

```sh
pqsio info sample.pairs.pqs
pqsio head sample.pairs.pqs -n 10
pqsio export sample.pairs.pqs -o sample.pairs.gz

pqsio bam2pairs hic.bam -o hic.pairs.pqs --threads 4
pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k
pqsio --help
```

## Documentation

- [Quick start](docs/index.md)
- [Installation](docs/installation.md)
- [Python API](docs/python.md)
- [CLI](docs/cli.md)
- [PQS format](docs/pqs-format.md)
- [Complete-read selection and deferred pair expansion](docs/concat-workflow.md)
- Other languages: [Rust](docs/rust.md), [C/C++](docs/native.md)

Preview with Zensical using `pixi run -e docs docs-serve`, then open
`http://localhost:2121`. See [development](docs/development.md),
[documentation maintenance](docs/documentation.md) and [changelog](CHANGELOG.md)
for contributor information.
