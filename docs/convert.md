# Convert concat to pairs

CLI: `pqsio convert sample.concat.pqs -o sample.pairs.pqs --min-mapq 1 --threads 4`.
See [CLI setup and options](cli.md).

```python
from pqsio import convert
result = convert("sample.concat.pqs", "sample.pairs.pqs", mode="concat2pairs",
                 min_mapq=1, min_order=2, max_order=None, threads=4)
print(result.to_dict())
```

The default `concat2pairs` mode accepts concat PQS input and writes pairs PQS. Rust exposes
`convert(input, output, "concat2pairs", ConvertOptions::default())`; C/C++ can
call `pqsio_convert_json` from `pqsio.h`. [BAM and PAF import modes](import.md)
use the native Rust `import_alignments` API and the additive `pqsio_import_json`
C ABI, also exposed through Python/CLI.
The separate [pairs2cool mode](cool.md) accepts pairs PQS or text and writes
single-resolution Cooler using Rust `pairs2cool` / C `pqsio_pairs2cool_json`.
`threads=1` is the default. Conversion reads `ConcatColumns` and builds packed
`PairColumns` directly, without materializing Alignment/Pair objects or allocating
a String for each pair ID. Midpoints are calculated once per selected alignment.
Read ID decimal bytes are encoded once per read; alignment-index decimal bytes
are cached per worker. The `read_id:left_index:` prefix is assembled once per
left endpoint, then copied with the cached right index into the packed ID column.
Worker-local selection, midpoint, index and prefix buffers persist across jobs.
Output column buffers are reused synchronously or returned from the writer
through a nonblocking, one-slot recycle queue per conversion worker.
Larger thread values parallelize pair expansion across complete reads within
each input batch and enable a persistent Parquet encoding/writing pool. Shard
numbers are assigned in source order, preserving pair IDs, order and counts.
Rust uses `ConvertOptions.threads`; C/C++ uses
`pqsio_convert_parallel_json` with an additional threads argument. The original
C entry point remains compatible and uses one worker.
It scans q0, filters alignments by `min_mapq`, then selects reads with
`min_order <= order < max_order` (`None` means no practical upper limit in Python).
Alignments are stably sorted by query start and expanded into all `n*(n-1)/2`
pairs, including same-contig pairs. Pair MAPQ is the minimum of both ends.
Positions use `start + (end - start) // 2 + 1` (1-based); this explicitly adds
one compared with CPhasing's legacy midpoint conversion. Pair IDs are
`logical_read_id:left_index:right_index`, with zero-based sorted indices after
filtering, deterministic across batch/chunk sizes. Contigs and explicit `cn.info`
are preserved; other sidecars are not copied. q1 and counts are regenerated.

`chunk_size=1_000_000` and `batch_rows=65_536` bound output buffers, not total
memory: decoded row groups and a complete oversized read may exceed these
targets. Pair expansion is quadratic in read order but is never fully buffered.
Workers use bounded queues: each holds at most one queued output batch and one
batch being built/sent, plus one batch at the writer. Output buffering therefore
grows with `threads * batch_rows`. Recycling can retain one additional spare
output buffer per worker; scratch arrays retain capacity for that worker's
largest selected read until conversion ends. For `threads > 1`, the encoder additionally
holds at most `threads` in-flight shards of up to `chunk_size` rows, alongside
the current writer shard and Parquet workspaces. Smaller chunks reduce memory
but increase file count. Input decoding and publication remain sequential;
individual shards encode/write concurrently. All encoder jobs finish before
publication, and worker errors prevent publication and clean up staging.
One oversized read is not split among workers. Conversion workers are created
once per conversion and reused across input batches. Input columns are shared
through reference counting, without cloning their buffers. Only one input batch
is dispatched at a time, with bounded per-worker task/output queues. Tiny batches
or few reads still offer limited parallelism. `threads=N > 1` allows up to N
persistent conversion workers plus N persistent
encoding workers; it is not a total CPU thread cap and does not configure
Polars' internal pool.

Existing output is rejected; failed conversions clean up their staging directory.
Inputs must remain unchanged during conversion. As with merge, callback failure
after publication leaves the valid output in place.
