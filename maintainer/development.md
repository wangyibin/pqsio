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

## Release acceptance scope

Freeze new features for this source release. Acceptance covers Linux x86-64
using the locked Pixi environment, native pairs/concat storage, CLI conversions
and the Python/C/C++ bindings. Storage schemas and ABI v1 remain unchanged.
See the [compatibility boundary](../docs/storage.md#release-compatibility-boundary)
before using an external reader.

CPhasing/PyArrow 10.0.1 concat interoperability is explicitly excluded, not
fixed: the integration suite must continue to expose its footer-decoding
failures. Passing pairs fixtures does not establish full CPhasing pipeline
compatibility. Other PyArrow versions, aarch64, other operating systems and
prebuilt Python/Conda packages need separate acceptance.

## v0.2.5 source-release acceptance

Validated with the same Linux x86-64 environment and compatibility scope below:

- `PQSIO_COOLER_TEST_PYTHON=/path/to/consumer/python pixi run --locked test`:
  53 Rust tests and 189 Python/C/C++ cases passed, no skips; one opt-in Rust
  performance test ignored.
- Strict Clippy, documentation build, Markdown file links, actionlint workflow
  validation and version consistency checks passed. CLI and Python report 0.2.5.
- Both maintenance documents are retained outside `docs/`; their website pages
  are absent from the generated site.
- CPhasing compatibility: 8 passed, 2 known concat errors with PyArrow 10.0.1,
  outside the supported scope.
- Offline crate file inventory passed. Full extracted-package build verification
  is configured in the publishing workflow; it was not rerun locally for 0.2.5.
  No crates.io upload, GitHub Pages deployment, full genome pipeline or final
  release artifact was performed locally.

## v0.2.4 source-release acceptance

Validated with the same Linux x86-64 environment and compatibility scope below:

- 53 Rust tests and 189 Python/C/C++ cases passed with the independent Cooler
  consumer enabled; no skips, one opt-in Rust performance test ignored.
- Strict Clippy, documentation build, version consistency and README hosted-link
  target checks passed. CLI and Python both report `0.2.4`.
- CPhasing compatibility: 8 passed, 2 known concat errors with PyArrow 10.0.1,
  outside the supported scope.
- crates.io publication and publication dry-run tasks were added but not run;
  no full genome pipeline or final release artifact was validated.

## v0.2.3 source-release acceptance

Validated with the same Linux x86-64 environment and compatibility scope below:

- `PQSIO_COOLER_TEST_PYTHON=/path/to/consumer/python pixi run --locked test`:
  53 Rust tests and 189 Python/C/C++ cases passed, no skips; one opt-in Rust
  performance test ignored.
- Strict Clippy, polling-based documentation build and internal link checks passed.
- Synthetic Python quickstart examples executed; the complete Rust quickstart
  type-checked against v0.2.3. CLI and Python both report `0.2.3`.
- CPhasing compatibility: 8 passed, 2 known concat errors with PyArrow 10.0.1,
  outside the supported scope. No full genome pipeline was run.

## v0.2.2 source-release acceptance

Validated on the v0.2.2 sources before committing and tagging the source release.
Acceptance covers the scope above, including the complete-read concat workflow,
CLI help improvements and documentation updates. Storage schemas and ABI v1
remain unchanged.

Environment: Linux x86-64, locked Pixi, Rust/Cargo 1.91.1, Python 3.11.16 and
Polars 0.20.29; all native builds used `dev-release`.

| Check | Result |
| --- | --- |
| `PQSIO_COOLER_TEST_PYTHON=/path/to/consumer/python pixi run --locked test` | Exit 0; 53 Rust tests passed, 1 opt-in performance test ignored; all 189 Python/C/C++ cases passed with no skips |
| Independent Cooler consumer | Python 3.8.8, Cooler 0.9.1, h5py 3.8.0; all 10 Cooler tests ran in the main suite |
| `pixi run --locked lint` | Exit 0; all-target Clippy with warnings denied |
| `pixi run --locked -e docs docs-build` | Passed with the polling watcher; generated homepage and updated documentation pages verified |
| Separate CPhasing compatibility suite | 10 tests: 8 passed, 2 concat errors (`Unrecognized type:24`); outside the supported scope, not an all-green integration result |
| CLI/Python version smoke checks | Both report `0.2.2` |
| `git diff --check` | Passed |

The independent Cooler Python reads generated files; it is not the supported
Python binding environment. The three new concat workflow tests cover complete
alignment preservation, independent MAPQ/order filtering, deferred expansion,
real-input logical-ID selection, empty results and output-path safety.

The separate CPhasing checks used the sibling source checkout, Python 3.8.8,
PyArrow 10.0.1 and Polars 0.20.29 with the newly built pqsio shared library.
Both synchronous and parallel pairs checks passed; both concat checks failed.
The CPhasing Rust-writer fixture and independent Polars fixtures passed.
The available samtools enabled all BAM fixture tests. Historical v0.0.1
capability tests and the CPhasing copy-number loader check ran without skips.

The host file-watch limit was 8192 (`fs.inotify.max_user_watches`). A syscall
trace confirmed `ENOSPC` during earlier documentation builds. Both Pixi
documentation tasks now set `ZENSICAL_POLL_WATCHER=1`, allowing build/preview
without administrator permissions. See
[file-watch troubleshooting](documentation.md#troubleshooting-an-empty-site).

No full genome pipeline, fresh-machine install, aarch64 runtime, alternate
Python/PyArrow matrix, prebuilt wheel/Conda package or final `release`-profile
artifact was validated. Use a new tag for new source revisions; do not repoint
an existing release tag.

## v0.1.0 validation

Validated on Linux x86-64 with `pixi run --locked test`: 39 Rust tests and
112 Python/C/C++ tests passed. One opt-in Rust performance test was ignored;
one CPhasing integration test was skipped because it expects the old sibling
checkout layout. No full CPhasing pipeline or aarch64 runtime was tested.
C/C++ round trips and Python readers used the same dev-release shared library.
`pixi run --locked -e docs docs-build` and local Markdown link checks passed.

## Source releases

`.github/workflows/cargo-publish.yml` validates and publishes version tags in
`wangyibin/pqsio`. Configure the repository Actions Secret `CARGO_REGISTRY_TOKEN`
with a crates.io token created by wangyibin, scoped to `pqsio` with
`publish-update` and, for the first publication, `publish-new`.
Manual runs default to dry-run mode. Publication verifies the extracted crate
before uploading; versions already published on crates.io cannot be replaced.


Keep `Cargo.toml`, the root pqsio entry in `Cargo.lock`, `pyproject.toml`,
`pixi.toml` and `python/pqsio/__init__.py` on the same library version.
Storage format versions and the C ABI version are separate contracts.

Before creating an annotated release tag:

1. Run `pixi run --locked test` and review any skipped tests.
2. Run `pixi run --locked lint`.
3. Run `pixi run --locked -e docs docs-build`.
4. Run the separate compatibility suite below; record each supported/unsupported
   path and the actual environment. Do not describe excluded failures as passes.
5. Review the Git changes, including existing documentation edits and deletions,
   and record the final validation results and all skips.
6. Commit source, headers, Python bindings, tests, licenses and lockfiles; keep
   environments, build outputs and generated native libraries ignored.
7. Tag the verified commit, then push the commit and tag to the intended remote.
   Do not move an existing tag to include later fixes.

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
