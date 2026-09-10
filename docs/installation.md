# Installation

From the `pqsio` repository directory:

```sh
pixi install --locked
pixi run build          # dev-release; native shared/static/Rust libraries
pixi run test           # standalone Rust, Python, C11 and C++17 tests
pixi run lint           # strict Clippy, using dev-release
```

`pixi.toml` manages the Rust 1.91 toolchain, C/C++ compilers and build tools,
plus Python 3.11 and Polars 0.20.29 for tests only. Rust library dependencies
remain in `Cargo.toml` / `Cargo.lock`, including Polars 0.49.1. The base Python
package still has no third-party runtime dependencies. `pixi.lock` pins the
development environments; `.pixi/` is local and ignored by Git.

Pixi sets `PYTHONPATH` and `PQSIO_LIBRARY` for the local package and
`target/dev-release/libpqsio.so`, plus four Polars/Rayon threads. Use
`pixi run python` for an interpreter configured to use the development library.
The standalone test task does not require the sibling CPhasing checkout.

Individual tasks are `test-rust`, `test-python`, `test-columns`, `test-native`,
`test-parallel`, `test-streaming`, `test-query`, `test-inspection`, `test-merge`,
`test-subset`, `test-copy-numbers`, `test-convert`, and `test-decode`. Python/native tests build the shared library first. `pixi run bench-columns --rows
80000 --repetitions 5`, `pixi run bench-streaming` and `pixi run bench-query`
explicitly run synthetic benchmarks; normal builds/tests do not
run them. `pixi run build-release` is reserved
for final release artifacts under `target/release/`; development and debug work
use `dev-release`. To use release libraries in Python, explicitly override
`PQSIO_LIBRARY` after activation.

From the parent workspace, use `pixi run --manifest-path pqsio/pixi.toml build`.
The manifest targets Linux x86-64 and aarch64; platform validation is documented
below. First installation/build needs cached packages or network access.
Cargo tasks use `--locked`; add `CARGO_NET_OFFLINE=true` when all Rust crates are
cached and offline operation is required. Use `pixi install --locked` to verify
reproducibility; run `pixi lock` deliberately when changing development dependencies.
