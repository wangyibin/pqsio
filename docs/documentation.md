# Documentation maintenance

The site uses [Zensical](https://zensical.org/) with `zensical.toml`.
Use its default modern appearance and automatic/light/dark color switcher.
English is the current documentation language. Keep the main navigation focused
on Installation, Python API and CLI; put advanced topics under Reference.

## Local preview and build

Run from the repository root. Pixi manages Zensical in the independent
`docs` environment (Linux x86-64):

```sh
pixi install --locked -e docs
pixi run -e docs docs-serve
# Build static HTML:
pixi run -e docs docs-build
```

Preview defaults to `http://localhost:2121`. Static output goes to `site/`.
The build task runs `zensical build --strict --clean -f zensical.toml` and
checks that `site/index.html` exists and is nonempty.
Press Ctrl+C to stop the preview. To use a different address or port, run
`pixi run -e docs zensical serve -f zensical.toml -a localhost:8001`.
Documentation dependencies are locked in `pixi.lock` and separate from library
runtime dependencies and the native development environment. No separate pip
installation or virtual environment is needed.

## Troubleshooting an empty site

If Zensical reports a successful build but produces no homepage, or the preview
returns 404, check Linux file-watch resources. An `inotify_add_watch` failure
with `ENOSPC` means the user has exhausted the available file watches; it does
not mean the documentation output directory is full. Close unnecessary file
watchers, or ask an administrator to raise the limit, for example:

```sh
sudo sysctl -w fs.inotify.max_user_watches=524288
```

This system-wide setting lasts until reboot. After resources are available,
rerun `pixi run -e docs docs-build`, then `pixi run -e docs docs-serve`.

## Editing

Keep README concise and put detailed usage in `docs/`. Add each new page to
`project.nav` in `zensical.toml` and use relative Markdown links between documentation pages.
Build the site after changes and check links. No deployment is configured yet.

## Benchmark records

Publish concise benchmark figures or tables with a reproducible script, input description,
environment and measurement limits. Keep raw JSON measurements and console logs
under ignored `tests/output/`. Run benchmarks only when needed.

The [PQS format page](pqs-format.md#performance-benchmark) uses SVG figures from
`docs/assets/benchmarks/format-summary.csv` for reads and
`docs/assets/benchmarks/format-write-summary.csv` for writes. Regenerate them with Python and
Matplotlib available (neither plotting nor Matplotlib is required for a site build):

```sh
MPLCONFIGDIR=tests/output/format-figures/mpl-cache python scripts/plot_format_bench.py
# Refresh both CSVs, figures and exact-value tables from complete measured reports:
MPLCONFIGDIR=tests/output/format-figures/mpl-cache python scripts/plot_format_bench.py \
  --report tests/output/format-compression/read.json \
  --write-report tests/output/format-compression/write.json \
  --update-page docs/pqs-format.md \
  --preview-dir tests/output/format-figures
```

The summaries contain four variants: Text, Gzip level 6, uncompressed PQS and
default PQS. Blue compares Text with uncompressed PQS; green compares Gzip with
default PQS. Speedup is comparison time divided by the corresponding PQS time:
higher is faster. Resource ratios use the same pairing; higher means PQS uses less.

Run read and write benchmarks sequentially as described on the format page.
`--update-page` replaces only the marked exact-value tables. The main figure summarizes one million records; detailed figures retain all
scales and resource measurements. Review the leading findings and method
manually against the new reports. Published summaries and
figures must not contain source names or local paths. Keep raw per-run reports
under ignored `tests/output/`; small pilot runs are not published measurements.
