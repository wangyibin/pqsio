# Ordered streaming merge — 0.0.9

```python
import pqsio
result = pqsio.merge(["a.pqs", "b.pqs"], "merged.pqs",
                     chunk_size=1_000_000, batch_rows=65_536,
                     provenance=True)
print(result.to_dict())
```

Rust: `merge(&inputs, output, MergeOptions::default()) -> anyhow::Result<MergeResult>`.
The public result contains `output`, `kind`, `counts`, `sources`, `provenance`,
and supports `to_value()` / `to_json()`. Python's `MergeResult.to_dict()` returns
an independent dictionary with `output`, `format`, `counts`, `sources`, and
`provenance_files`. Counts include q0/q1 records and concat reads (zero for pairs).
Source summaries include observed q0 input/output record counts, output read
counts (zero for pairs), and omitted application sidecars. Source counts in JSON
are decimal strings, matching the provenance schema; aggregate result counts are
JSON integers (Python preserves their precision).

This is an additive API; existing Reader/Writer and ABI v1 symbols are unchanged.
Python calls the new `pqsio_merge_json` C symbol and reports an actionable error
with older native libraries. The base package uses only the standard library.
No dependency or storage-format changes are required. There is no CLI.

## Semantics and validation

Inputs retain caller order. Each input uses the existing Reader shard discovery
and numeric filename ordering, then physical row order. Contigs are unified in
input/contig/first-occurrence order, including unused declarations. Equal names
require equal lengths; conflicts identify both sources and lengths. Names are
literal, including `chr` prefixes. All chromosome columns are remapped; Writer
selects the position dtype from the union, including UInt64 for long coordinates.
All other core fields and coordinates are preserved, except concat read IDs.

Pairs IDs are preserved and **are not guaranteed globally unique**. Duplicate
records remain. Concat reads receive new UInt64 IDs from 1, in logical traversal
order. Input boundaries always separate reads. Shard-local IDs are separate in
each shard; global IDs can continue across adjacent shards/row groups. IDs must
be nondecreasing in their scope, and each complete read must have a consistent
read length. Decreases/reappearing groups fail instead of being sorted or cached.
New read IDs and per-source counters are checked for overflow. Writer receives complete reads;
one oversized read occupies one oversized shard.

Only q0 is read, with no MAPQ filtering. Writer regenerates q1 using MAPQ >= 1,
including its counts. Input q1 differences, even corrupt q1 files, are not
propagated. **Merge is not a full validate** and does not certify input q1,
declared counts, all metadata declarations, or unrelated application files.

Before creating staging, merge checks nonempty input list, positive options,
supported pairs 0.1.0 / concat 0.2.0 and matching kinds, readable metadata,
contigs and q0/q1 directories, nonempty positive-length contig tables, contig
conflicts, and path safety. An explicitly present `read_idx_scope` must be
`global` or `shard`; missing scope keeps Reader's legacy global interpretation.
Explicit unknown concat `record_type` / `partition_key` or non-true
`is_with_mapq` is rejected. Other metadata fields are not copied or treated as
new supported semantics. Declared complete-group flags do not replace streaming
checks; adjacent-shard global continuations are supported.

Canonical paths resolve relative and symlink input aliases; repeated inputs are
rejected. Existing outputs/staging paths, including dangling symlinks, are
rejected. Output and its `.partial` sibling may not overlap an input directory
or its resolved q0/q1 directories. The output parent must already exist.
Prechecks do not decode Parquet or perform full validation. During streaming,
the existing decoder and column Writer check core fields, unknown contigs,
null/numeric values, positions, strands, read intervals and read grouping.

Inputs must remain unchanged during the call; no snapshot consistency under
concurrent modification is promised. All input access is read-only.

## Provenance schema (version 1)

With provenance enabled, the following UTF-8 JSONL sidecars are created in
Writer-owned staging, flushed and closed before `finish()` publishes output.
JSON escaping handles arbitrary supported strings. All IDs and record/read
counts below are **decimal strings**, preserving the full UInt64 range across
JSON implementations. Source indices are zero-based JSON integers.

`_merge_sources.jsonl`: one line per input, including empty inputs:

```json
{"schema_version":1,"source_index":0,"path_label":"a.pqs","format_version":"0.2.0","input_read_id_scope":"shard","input_records":"8","output_records":"8","output_reads":"3","omitted_sidecars":["cn.info"]}
```

`input_read_id_scope: null` means absent metadata (implicit legacy global).
Path labels use caller spelling; Rust non-UTF-8 path labels use lossy Unicode
rendering. Source indices remain unambiguous. Omitted sidecars list immediate
input entries other than regenerated standard files and q0/q1, including older
merge sidecars. No recursion or application-specific interpretation is done.
`_readme`, metadata and counts are regenerated, not copied.

`_merge_reads.jsonl`: concat only, one line per output logical read:

```json
{"source_index":0,"output_read_id":"1","logical_read_id":"3","raw_read_id":"7","shards":["q0/1.parquet"]}
```

- `output_read_id`: newly allocated global ID.
- `logical_read_id`: the existing streaming Reader's ID, before merge remapping
  (raw ID for global inputs; deterministic remapped ID for shard-local inputs).
- `raw_read_id`: captured from the decoded disk column **before** Reader mapping.
- `shards`: source-relative shard paths traversed by this read. Shard-local reads
  have one shard; together with source index and raw ID this identifies the
  original read. Global continuations can list multiple adjacent shards.

The mapping is streamed, never accumulated for the whole dataset. Disabling
provenance generates neither sidecar and does not capture origin columns.
No `cn.info` or other application sidecars are copied/merged; their names remain
in the result even when provenance is disabled. This is not lossless merging of
arbitrary PQS application content.

## Memory and failure handling

There are no new parallel workers, whole-dataset row buffers, global sorting,
or unbounded sets of previously seen read IDs. Existing Parquet encoding may
use its normal library thread pool. Reader decodes one complete row group;
`batch_rows` controls returned column batches, not decoder allocation. Complete
concat reads may exceed it, with one read of lookahead. Writer buffers up to
`chunk_size` records or one oversized read. Peak memory also includes temporary
DataFrames, encoding/filtering buffers, string storage and column copies.
Chromosome/ID remapping mutates owned columns; append/filter/decode/write paths
still allocate and copy. This is not end-to-end zero copy or a fixed RSS bound.
Contig maps, input summaries, and existing per-input shard filename lists also
consume memory proportional to those metadata sizes. Origin shard lists grow
with the shards occupied by the current read, not all reads.

Writer owns the staging directory and fixed merge-sidecar creation. On normal
read/validation/write/provenance errors, file handles close and Writer Drop
removes only staging it successfully created; no final output is published.
Preexisting paths are never cleaned up or overwritten. Publication uses the
existing destination reservation and rename protocol after all data, metadata,
counts and sidecars complete. No stronger fsync/crash-durability guarantee is
added. Native result callbacks run after publication: if a C callback rejects
the result, the error explicitly says the output was already published and is
valid. This result-delivery error is distinct from a merge/provenance failure.

## Focused checks

From `pqsio`: `pixi run test-merge` exercises the Python/C bridge against native
Rust and small synthetic datasets. `pixi run test-rust` includes the public Rust
API and overflow check. Existing streaming tests cover the shared decoder.
