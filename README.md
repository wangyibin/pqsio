# pqsio

Reusable Rust storage for **pairs PQS 0.1.0** and **alignment-level concat PQS
0.2.0**, with a C ABI, C++17 wrappers and a dependency-free Python binding.
The library is independent of CPhasing’s phasing and alignment algorithms.
The library release is **v0.1.0**; storage format versions are independent.
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

## Documentation

Start with the [documentation home](docs/index.md) or jump to:

- [Installation](docs/installation.md)
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
