# Installation

## Recommended: Pixi

Requirements: Linux (x86-64 or aarch64), Git and [Pixi](https://pixi.sh).
The aarch64 environment is configured but has not been validated at runtime.
The first install/build needs network access unless dependencies are cached.

```sh
git clone https://github.com/wangyibin/pqsio.git
cd pqsio
pixi install --locked
pixi run build
```

If you already have the source, run the last two commands from the `pqsio`
directory. Pixi installs the build tools and Python dependencies, builds the
native Rust executable and library, and configures the Python library path automatically.

Check the installation:

```sh
pixi run pqsio --version
pixi run python -c 'import pqsio; print(pqsio.__version__)'
```

Run commands and scripts from this repository with:

```sh
pixi run pqsio --help
pixi run python example.py  # Save the homepage example as example.py first.
```

Continue with the [Python API](python.md) or [CLI guide](cli.md).

## Use the standalone Rust command

After building, add the binary directory to PATH:

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
