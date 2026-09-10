# Documentation maintenance

The site uses MkDocs with Material and `mkdocs.yml`, following
CPhasing’s explicit navigation, light/dark palettes and code-copy controls.
English is the current documentation language.

## Local preview and build

Run from the repository root. Pixi manages Python and MkDocs in the independent
`docs` environment (Linux x86-64), following CPhasing's docs feature:

```sh
pixi install --locked -e docs
pixi run -e docs docs-serve
# Build static HTML:
pixi run -e docs docs-build
```

Preview defaults to `http://localhost:2121`. Static output goes to `site/`.
The build task checks that `site/index.html` exists and is nonempty.
Press Ctrl+C to stop the preview. To use a different address or port, run
`pixi run -e docs python -m mkdocs serve -f mkdocs.yml -a localhost:8001`.
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
`nav` in `mkdocs.yml` and use relative Markdown links between documentation pages.
Build the site after changes and check links. No deployment is configured yet.

## Benchmark records

Keep reproducible scripts and focused tests, rather than historical timing tables,
raw JSON measurements or console logs in the documentation. Run benchmarks only
when needed, and store local outputs under ignored `tests/output/`.
