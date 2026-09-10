# Development and validation

```sh
# From pqsio; these tasks configure paths and build the development library:
pixi run test
pixi run lint
```

Rust and standalone Python/C/C++ tests cover row/column cross-path I/O, long
positions, MAPQ filtering, complete/oversized concat reads, invalid input,
buffer lifetimes, abort cleanup and output collisions. Independent Polars
fixtures in the column tests cover legacy shard-local read IDs and disk schemas.
The historical-library test skips when its named archived v0.0.1 library is
absent from a fresh checkout.

Native tests honor activated `CC`/`CXX` (falling back to `cc`/`c++` outside
Pixi). C/C++ consumers link the directory containing `PQSIO_LIBRARY`; the Python
binding loads that same absolute library path and reads the native outputs.
The aarch64 environment is resolved in the lockfile but requires separate
build and runtime validation.

## v0.1.0 validation

Validated on Linux x86-64 with `pixi run --locked test`: 39 Rust tests and
112 Python/C/C++ tests passed. One opt-in Rust performance test was ignored;
one CPhasing integration test was skipped because it expects the old sibling
checkout layout. No full CPhasing pipeline or aarch64 runtime was tested.
C/C++ round trips and Python readers used the same dev-release shared library.
`pixi run --locked -e docs docs-build` and local Markdown link checks passed.

## Source releases

Keep `Cargo.toml`, the root pqsio entry in `Cargo.lock`, `pyproject.toml`,
`pixi.toml` and `python/pqsio/__init__.py` on the same library version.
Storage format versions and the C ABI version are separate contracts.

Before creating an annotated release tag:

1. Run `pixi run --locked test` and review any skipped tests.
2. Run `pixi run --locked -e docs docs-build`.
3. Review the Git changes, including existing documentation edits and deletions.
4. Commit source, headers, Python bindings, tests, licenses and lockfiles; keep
   environments, build outputs and generated native libraries ignored.
5. Tag the verified commit, then push the commit and tag to the intended remote.

A Git source release does not bundle a compiled shared library. Python users
must build/provide the native library and set `PQSIO_LIBRARY`; prebuilt Python
or Conda distribution requires a separate packaging step. Only build final
native release artifacts with the explicit `build-release` task.

The additional `tests/test_compatibility.py` integration suite requires a
separately configured full CPhasing environment (its Python dependencies and
optionally its Rust executable), rather than the standalone Pixi environment.
Run it from the parent workspace with that environment's Python:

```sh
PQSIO_LIBRARY="$PWD/pqsio/target/dev-release/libpqsio.so" PYTHONPATH=pqsio/python:CPhasing python -m unittest discover -s pqsio/tests -p test_compatibility.py -v
```
