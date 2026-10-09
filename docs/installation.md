# Installation

## Bioconda

```sh
conda create -n pqsio --override-channels -c conda-forge -c bioconda \
  --strict-channel-priority pqsio
conda activate pqsio
pqsio --version
```

Channels follow the [Bioconda guide](https://bioconda.github.io/index.html#with-conda).
No administrator access is needed.

## Build from source with Pixi

Requires Linux, Git and [Pixi](https://pixi.sh). Linux x86-64 is validated;
aarch64 is configured but not runtime-tested.

```sh
git clone https://github.com/wangyibin/pqsio.git
cd pqsio
pixi install --locked
pixi run build
export PATH="$PWD/target/dev-release:$PATH"
pqsio --version
```

If the source is already present, start from `cd pqsio`. Repeat the PATH setting
in new shells, or save the absolute binary directory in your shell configuration.
The first build needs network access unless dependencies are cached.

## Python after a source build

From the source repository, run examples with `pixi run python example.py`.
To use another Python environment (Python >= 3.9):

```sh
python -m pip install .
export PQSIO_LIBRARY="$PWD/target/dev-release/libpqsio.so"
python -c 'import pqsio; print(pqsio.__version__)'
```

`pip install .` installs the Python bindings; build the native library first.
Set `PQSIO_LIBRARY` in each new shell. The CLI uses the native executable on PATH.

Continue with [CLI](cli.md), [Python](python.md) or [Rust](rust.md).
For external readers, check [compatibility](storage.md).
