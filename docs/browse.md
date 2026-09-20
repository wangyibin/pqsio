# Browse and export

Browse a PQS directory and export interoperable text without moving record
processing into Python. All decoding, filtering, text formatting and compression
run in Rust. The native Rust CLI supplies help, tables and progress.

## Summary

```sh
pqsio info sample.pairs.pqs
pqsio info sample.pairs.pqs --json
pqsio info sample.concat.pqs --stats --progress
```

The default reads metadata and Parquet footers, not record pages. It reports
format/version, coordinate convention, chromosome count, q0/q1 record counts,
shard count and total Parquet bytes. Size excludes metadata and indexes.
q1 is the redundant MAPQ > 0 partition: do not add it to q0.
Concat read counts in the default table are metadata declarations.

`--stats` scans q0 once for an exact 256-entry MAPQ histogram and, for concat,
logical read count. The table groups MAPQ as 0, 1–9, 10–19, 20–29 and >=30;
JSON retains all bins. This scan can take time on large datasets.

## Preview and filtering

```sh
pqsio head sample.pairs.pqs                 # 10 data rows plus a TSV header
pqsio head sample.concat.pqs -n 20
pqsio view sample.pairs.pqs                 # at most 100 data rows
pqsio view sample.pairs.pqs --all --min-mapq 30 \
    --region chr1:1000000-2000000 --columns readID,chrom1,pos1,chrom2,pos2
pqsio view sample.pairs.pqs --all --no-header | head -n 10
```

Both commands support `-n/--limit`, `--columns`, `--min-mapq`, repeatable
`--region CHROM:START-END`, `--pairs-mode either|both` and `--no-header`.
`-n 0` emits just the header, unless suppressed. `view --all` overrides its
preview limit. Selected columns retain the requested order. Chromosome fields
contain names, not dictionary codes; leading zeros in string read IDs survive.

Regions are **0-based half-open** for either format. Exported pairs positions
remain **1-based**, and concat intervals remain **0-based half-open**.
Pairs default to either endpoint matching; `both` requires both endpoints to
match the selected region union. Concat emits matching alignments, not complete
reads. Multiple regions use the existing query/index machinery and its scan
fallback. MAPQ filtering uses the existing quality partition selection; it
does not concatenate q0 and q1.

Limits count records, so concat reads may be cut by a preview limit. The native
reader stops requesting batches once the limit is reached, but may decode a
whole Parquet row group. `--columns` selects output fields; it does not yet
project Parquet columns. Memory remains subject to row-group size.

## Text export

```sh
pqsio export sample.pairs.pqs -o sample.pairs.gz --threads 4
pqsio export sample.concat.pqs -o sample.concat.gz
pqsio export sample.concat.pqs -o sample.concat.tsv.gz --format tsv
pqsio export sample.pairs.pqs -o positions.tsv --columns chrom1,pos1,chrom2,pos2
pqsio export sample.pairs.pqs -o - --format pairs | gzip > sample.pairs.gz
```

Export accepts the same filters, optional `-n/--limit` (default unlimited),
`--format auto|pairs|concat|tsv` and `-t/--threads` (default 1). `auto` chooses standard
pairs or concat text according to the input kind when exporting all columns;
with selected columns it chooses TSV.
Explicit `pairs` requires pairs input and all columns; `concat` requires concat
input and all 11 columns. Concat text matches cphasing-rs: no header, the column
order below, and unchanged 0-based half-open coordinates. TSV supports both PQS
formats. File suffix `.gz` or `.mgz` enables gzip-compatible compression through
the common Rust writer; `threads` controls compression workers. It does not
parallelize Parquet decoding for this operation.

| Format | Columns in default order |
| --- | --- |
| Pairs | `readID chrom1 pos1 chrom2 pos2 strand1 strand2 mapq` |
| Concat | `read_idx read_length read_start read_end strand chrom start end mapping_quality identity filter_reason` |

Pairs headers include format version, `#shape: whole matrix`, chromosome sizes
and `#columns`. Input order and endpoint orientation are preserved; export does
not sort or canonicalize records. TSV has a single tab-separated column header.
`--no-header` suppresses pairs/TSV headers. Concat never writes a header, even
with the API default `header=True`. Control characters in exported fields
are rejected because they cannot be represented faithfully as plain TSV.

File targets and their `.partial` staging paths must be absent, and the parent
must exist. Output inside the input PQS directory is rejected. Compression
finishes before the completed file is published; failures clean up the owned
staging file and never replace an existing target.

`-o -` sends uncompressed data to stdout without a JSON report. Other exports
print a JSON report with `format`, `columns`, `records` and `output`. Progress
and errors stay on stderr. Stdout streams may already contain partial data when
an error occurs; a downstream closed pipe exits quietly.

## Progress

All CLI operations accept `--progress/--no-progress`. Progress is enabled by
default when stderr is a terminal. Explicit `--progress` also emits it in
redirected logs; `--no-progress` disables it. The UI reports elapsed time and,
on success, stage durations. Conversion and browsing report native stages;
inspect, validate, subset and merge currently show operation-level elapsed time.

Counters belong to the current stage (rows, read groups or pixels), and totals
are shown only when known. A spinner is used for unknown totals. The UI does not
invent a percentage or ETA. Python API calls do not start a progress display.

## Python API

```python
import pqsio

summary = pqsio.info("sample.pairs.pqs", stats=False).to_dict()
pqsio.view("sample.pairs.pqs", limit=10, columns=["chrom1", "pos1"])
report = pqsio.export("sample.pairs.pqs", "sample.pairs.gz", threads=4)
print(report.to_dict())
```

```python
info(path, *, stats=False)
view(input, *, limit=100, columns=None, **options)
export(input, output=None, *, format="auto", columns=None, limit=None,
       min_mapq=0, regions=None, pairs_mode="either", header=True,
       threads=1, batch_rows=65536, index="auto", filter_mode=None, auto_index=False)
```

All return report objects with `.to_dict()`. `view` uses TSV and accepts export
filter/read options; `limit=None` streams all matches. `output=None` or `"-"`
writes directly to **OS stdout**, not a Python `StringIO` or redirected
`sys.stdout`. Use file output when a Python application needs to capture text.
For region queries, `index` accepts `auto`, `off`, or `require`; non-default
index modes require regions. Concat `filter_mode` accepts `matching_alignments`
or `complete_reads` (default `None` means matching). Reports contain
`query_stats` for region operations, otherwise null. See [query CLI and index
management](query.md) for the full semantics and dedicated `query` command.

## Rust and C ABI

Rust exposes `pqsio::presentation::{info, export, ExportOptions}`. `info(path,
false)` returns a JSON `Value` with `.json()`. `export(path, Some(output_path),
ExportOptions::default())` writes a file; `None` writes stdout. `ExportOptions`
contains `format`, `columns`, `limit`, `header`, `threads` and `query: QueryOptions`.
Use `query` for MAPQ, regions, pairs matching mode and read batch size.

C exposes `pqsio_info_json(path, stats, callback, user)` (`stats` is 0 or 1) and
`pqsio_export_json(input, output, options_json, callback, user)`.
A null output means stdout; unlike the Python API, a literal `"-"` is a filename.
`options_json` is a required JSON object (use `"{}"` for defaults), with the
Python export option keys except `input`/`output`. Regions are arrays of
`[contig, start, end]`; `columns` and `limit` may be null. Unknown keys are errors.
Both functions return 0 or -1; export returns -3 for a broken pipe.
Result callbacks borrow length-delimited UTF-8 JSON; return 0 and do not reenter
or throw across the ABI. Callback failure after publication retains the file.

Optional `pqsio_set_progress_callback(callback, user)` registers one observer
on the calling thread, replacing any previous registration. Its arguments are
`stage_bytes, stage_length, completed, total, user`. Stage bytes are borrowed
only during the call; `total=0` means unknown. Keep the callback and user data
alive until clearing registration with a null callback **on the same thread**.
Callbacks are synchronous on that thread, never on decoding/compression workers;
do not reenter pqsio or unwind across the boundary. Rust callers can use unsafe
`pqsio::progress::set` with the same lifetime contract.
