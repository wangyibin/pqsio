# pqsio

Read, write and convert genomic contacts with a native CLI and Python, Rust,
C/C++ APIs. Supports pairs and multi-fragment concat PQS, BAM/PAF import and
Cooler output.

PQS offers selective column/quality reads, preserves multi-fragment reads for
later pair expansion, and supports batch-based processing across languages.
In one-million-record tests, default PQS counted MAPQ >= 30 records
**20.87–33.50× faster** and wrote **4.71–6.25× faster** than gzip level 6 text.
See [benchmark methods](https://wangyibin.github.io/pqsio/pqs-format/#performance-benchmark).

## Install

```sh
conda create -n pqsio --override-channels -c conda-forge -c bioconda \
  --strict-channel-priority pqsio
conda activate pqsio
```

For a source build, see [installation](https://wangyibin.github.io/pqsio/installation/).

## Use

```sh
pqsio bam2pairs hic.bam -o hic.pairs.pqs --min-mapq 30
pqsio info hic.pairs.pqs
pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k
pqsio --help
```

PQS datasets are directories. Output paths must be new.

## [Documentation](https://wangyibin.github.io/pqsio/)

- [CLI](https://wangyibin.github.io/pqsio/cli/)
- [Python API](https://wangyibin.github.io/pqsio/python/) · [Rust API](https://wangyibin.github.io/pqsio/rust/) · [C/C++](https://wangyibin.github.io/pqsio/native/)
- [Complete-read concat workflow](https://wangyibin.github.io/pqsio/concat-workflow/)
- [Format and reference](https://wangyibin.github.io/pqsio/reference/) · [Compatibility](https://wangyibin.github.io/pqsio/storage/)

See [development](docs/development.md) and [changelog](CHANGELOG.md).
