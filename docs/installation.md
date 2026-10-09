# Installation

## Bioconda

Create a dedicated environment and run the CLI directly:

```sh
conda create -n pqsio --override-channels -c conda-forge -c bioconda \
  --strict-channel-priority pqsio
conda activate pqsio
pqsio --version
pqsio --help
```

The channel order and strict priority follow the
[Bioconda installation guide](https://bioconda.github.io/index.html#with-conda).
These commands need no administrator privileges and do not modify your channel
configuration.

## Build from source with Pixi

Validated release target: Linux x86-64, Git and [Pixi](https://pixi.sh).
The aarch64 environment is configured but is outside the validated release
scope until build and runtime checks pass. Python metadata allows >= 3.9;
the locked test environment uses Python 3.11, so other Python versions are
not covered by this release acceptance.

CPhasing with PyArrow 10.0.1 cannot consume generated concat PQS. No minimum
compatible PyArrow version has been established; use the native pqsio reader
for concat within this release scope. See [compatibility](storage.md).
The first install/build needs network access unless dependencies are cached.

```sh
git clone https://github.com/wangyibin/pqsio.git
cd pqsio
pixi install --locked
pixi run build
export PATH="$PWD/target/dev-release:$PATH"
```

If you already have the source, run the install, build and PATH commands
from the `pqsio` directory. Pixi installs the build tools and Python dependencies, builds the
native Rust executable and library, and configures the Python library path automatically.

Check the installation:

```sh
pqsio --version
pixi run python -c 'import pqsio; print(pqsio.__version__)'
```

Run the CLI directly. For Python examples using the Pixi environment, run
from this repository:

```sh
pqsio --help
pixi run python example.py  # Save the homepage example as example.py first.
```

Continue with the [Python API](python.md) or [CLI guide](cli.md).

## Use the standalone Rust command

The installation commands above add the binary directory to PATH for the
current shell. In a new shell, repeat the following from the repository, or
add the equivalent absolute path to your shell startup configuration:

```sh
export PATH="$PWD/target/dev-release:$PATH"
pqsio --help
```

The `pqsio` executable parses arguments and runs operations directly in Rust.
It does not require Python, rich-click, ctypes, or `PQSIO_LIBRARY`. Build the
optimized release executable with `pixi run build-release` when preparing a
release; its path is `target/release/pqsio`. This is a native executable, not a
fully static distribution: system native libraries may still be required.

## Use an existing Python environment

After building with Pixi, activate your Python environment (Python >= 3.9
with pip) and run these commands from the `pqsio` repository:

```sh
python -m pip install .
export PQSIO_LIBRARY="$PWD/target/dev-release/libpqsio.so"
python -c 'import pqsio; print(pqsio.__version__)'
```

You can now use `import pqsio` in that environment. Set `PQSIO_LIBRARY` again
in new shells, or add it to your environment's activation script.

`pip install .` installs only the Python API, with no third-party Python runtime
dependencies. It does not build/install the Rust executable or shared library.
Build them with Pixi as above. `python -m pqsio` is an optional compatibility
launcher that execs the Rust executable; it performs no argument parsing or
record processing. It finds the repository's development executable, then PATH,
or uses an explicit `PQSIO_BINARY` path. Existing pip-generated `pqsio` launchers
should be removed by reinstalling the Python package; put the native executable
on PATH instead.

Build/test details: [development](development.md).
