# pqsio

Reusable Rust storage for **pairs PQS 0.1.0** and **alignment-level concat PQS
0.2.0**, with a C ABI, C++17 wrappers, ctypes-based Python bindings and a rich-click CLI.
The library is independent of CPhasing’s phasing and alignment algorithms.
The library release is **v0.2.0**; storage format versions are independent.
A source release does not imply availability on PyPI, crates.io or Conda channels.

## Quick start

From this repository:

```sh
pixi install --locked
pixi run build
pixi run test
```

The Python package requires the native library; the Pixi environment configures
`PYTHONPATH` and `PQSIO_LIBRARY` for local use.

The command-line interface provides direct conversion commands, `info`, `stats`,
`head`, `view`, `export`, `query`, `index`, `inspect`, `validate`, `subset` and
`merge`. The existing `convert --mode` interface remains available:

```sh
pixi run pqsio --help
pixi run pqsio bam2pairs hic.bam -o hic.pairs.pqs
pixi run pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k
pixi run pqsio stats hic.pairs.pqs --json
pixi run pqsio index build hic.pairs.pqs
pixi run pqsio query hic.pairs.pqs --region chr1:0-1000000 --index require
pixi run pqsio info hic.pairs.pqs
pixi run pqsio head hic.pairs.pqs -n 10
pixi run pqsio view hic.pairs.pqs --region chr1:0-1000000 --columns chrom1,pos1,chrom2,pos2
pixi run pqsio export hic.pairs.pqs -o hic.pairs.gz --threads 4
pixi run pqsio export reads.concat.pqs -o reads.concat.gz --threads 4
pixi run pqsio convert sample.concat.pqs -o sample.pairs.pqs --threads 4
pixi run pqsio convert hic.bam --mode bam2pairs -o hic.pairs.pqs
pixi run pqsio convert reads.bam --mode bam2concat -o reads.concat.pqs
pixi run pqsio convert reads.paf.gz --mode paf2concat -o reads.concat.pqs
pixi run pqsio convert reads.paf.gz --mode paf2pairs -o reads.pairs.pqs
pixi run pqsio convert hic.pairs.pqs --mode pairs2cool --bin-size 10k --threads 4 -o hic.10k.cool
```

Install the Python package with `python -m pip install .` to get the standalone
`pqsio` command; configure `PQSIO_LIBRARY` for the built shared library.
See [CLI usage and installation](docs/cli.md).
See [browsing and export](docs/browse.md) for column selection, summaries and
progress on stderr (automatic in terminals; override with `--progress` or `--no-progress`).
All conversions parse, group and write in Rust. BAM imports use `rust-htslib`
directly, without a `samtools` subprocess; see [BAM/PAF import rules](docs/import.md).
Both `bam2pairs` and `bam2concat` accept paired-end Hi-C or single-end long-read BAM.
`paf2pairs` writes pairs PQS directly from PAF/gzip PAF, without an intermediate
concat dataset. Its default positions are 1-based leftmost, matching `bam2pairs`.
`pairs2cool` accepts pairs PQS or pairs text (plain/gzip/mgzip) and writes a
single-resolution Cooler file, with parsing, binning, aggregation and HDF5 I/O
entirely in Rust. See [pairs to Cooler](docs/cool.md).

## Documentation

Start with the [documentation home](docs/index.md) or jump to:

- [Installation](docs/installation.md)
- [Command-line interface](docs/cli.md)
- [API overview and conversion entry points](docs/api.md)
- [BAM and PAF import](docs/import.md)
- [Pairs to Cooler](docs/cool.md)
- Language APIs: [Python](docs/python.md), [Rust](docs/rust.md), [C/C++](docs/native.md)
- [Streaming reads](docs/streaming.md) and [columnar I/O](docs/columnar.md)
- [Region queries](docs/query.md), [subset](docs/subset.md), [merge](docs/merge.md), [conversion](docs/convert.md)
- [Storage contracts](docs/storage.md) and [development](docs/development.md)
- [Changelog](CHANGELOG.md)

Preview the documentation using the independent Pixi `docs` environment:

```sh
pixi run -e docs docs-serve
# Build static HTML into site/:
pixi run -e docs docs-build
```

Open `http://localhost:2121`. Pixi installs the documentation dependencies
automatically; see [documentation maintenance](docs/documentation.md) for details.
