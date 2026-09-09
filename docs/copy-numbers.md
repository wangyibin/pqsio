# Optional copy numbers — 0.0.12

`cn.info` is the only authoritative CN store. It does not change PQS schemas,
records, coordinates, read IDs, q0/q1 counts or Reader/query behavior. No virtual
contigs or alignments are created. No contact weighting, collapsed-list inference,
private pipeline conversion or automatic repair is performed.

```python
import pqsio
with pqsio.PairsWriter('new.pqs', {'a': 100, 'b': 200},
                       copy_numbers={'a': 3, 'b': 1}) as writer:
    writer.write_batch([])
info = pqsio.read_copy_numbers('new.pqs')
print(info.present, dict(info.explicit), info.effective('a'))
pqsio.set_copy_numbers('new.pqs', {'a': 3})  # replace entire declaration table
pqsio.update_copy_numbers('new.pqs', {'b': 2})  # preserve a, replace b
```

Rust exports `read_copy_numbers`, `set_copy_numbers`, `update_copy_numbers`, and
`Writer::set_copy_numbers(&BTreeMap<String, u64>)`. The Writer method is an explicit
setting stage before `finish`; it validates all entries before writing. Python
PairsWriter/ConcatWriter accept optional `copy_numbers` at construction, before
accepting records. ParallelWriter has no CN setting API in this change.
The additive C functions and `pqsio_copy_number` descriptor are documented in
`include/pqsio.h`; existing descriptors/signatures are unchanged. Python checks
for new symbols on demand and reports missing native copy-number capability.
Old interfaces do not require these symbols.

## Representation and compatibility

Read whitespace-separated, exactly two fields per nonempty, noncomment line.
Leading whitespace is stripped; lines beginning with `#` are ignored. Values
are integers in **1..=18446744073709551615** (`u64` / `uint64_t`). Negative,
zero, overflowing, noninteger values and duplicate names (even equal values)
are rejected. Dataset reads reject names outside `_contigsizes`; `effective`
also rejects unknown names. Writes reject names that cannot round-trip through
the format (whitespace, NUL, or leading `#`). Output is UTF-8, sorted by contig
name, with tabs and a trailing newline per declaration. Comments/spacing are
not preserved.

`present` distinguishes a missing file from a legal empty file. `explicit`
preserves all and only declarations, including explicit CN=1. Missing declarations
have effective CN=1; they are never materialized into the explicit table.
`set({})` writes a legal empty file. `update({})` performs no write or creation.
Set replaces declarations; update reads/validates existing declarations and
replaces only supplied keys. CN=1 never deletes a key.

The inspected CPhasing Python loader uses unrestricted Python integers, whereas
cphasing-rs uses platform `usize`; the shared u64 range matches the currently
supported 64-bit Linux targets, not hypothetical 32-bit builds. Both existing
loaders ignore blank/comment lines and reject duplicates and CN<1. Their missing
file checks use `is_file`; pqsio deliberately diagnoses existing non-files and
broken links as unreadable. Existing Rust copy copies raw bytes; existing Rust
merge merges explicit values only and emits no file for an empty merged table.
pqsio retains existence when any input has a legal empty file, a deterministic
extension with identical effective CN values. Existing remapping/materialization
functions and collapsed-list update algorithms are not reused or changed.

## Propagation

Writer writes CN inside its owned staging directory. CN errors poison the Writer;
finish cannot publish after such an error and Drop cleans staging. Publication
occurs only after core files, counts, metadata and CN have completed.

Merge validates CN against its unified contig table before creating the Writer.
Equal explicit declarations coalesce. Different values fail with both input paths
and values, including explicit 1 versus 3. A missing declaration does **not** act
as an explicit 1 conflict: although it defaults to 1 in an individual dataset,
merge combines declarations. No inputs with CN means no output CN; any legal file,
even empty, produces output CN. Per-source `copy_numbers_propagated` in summaries
and provenance reports presence. CN is excluded from omitted sidecars.

Subset retains its full input contig table and therefore every explicit CN,
including contigs without output alignments and completely empty outputs. MAPQ,
regions, matching-alignments and complete-reads selection never estimate CN.
Missing CN stays missing; legal empty files stay present. Result/provenance
`copy_numbers_propagated` reports input CN presence. Other application sidecars
and indexes keep their existing omission policy.

## Inspection and validation

Inspection includes `copy_numbers`: presence, explicit table, declaration count,
default 1, and known contig names. Failed CN inspection reports `status=invalid`
or `unreadable` with an error instead of aborting other core inspection work.
Quick validation checks CN without scanning alignments. `CN_FORMAT`, `CN_VALUE`,
`CN_DUPLICATE`, `CN_UNKNOWN_CONTIG` carry file, one-based line in `row`, contig in
`field`, and explanatory message. Unreadable CN yields `CN_UNREADABLE` and an
incomplete check; confirmed malformed CN yields invalid. Missing CN is legal.
Only the first CN error is returned; independent core checks continue.

## Update safety and limits

Set/update validate all supplied entries before changing anything, then create an
exclusive temporary file in the dataset directory, flush it, and rename it over
`cn.info`. Failure cleans this operation's temporary file and preserves the old
file. A hard-linked CN is replaced at this directory entry, never overwritten in
place. Symbolic links at CN or in the dataset directory path are refused for
updates; use the actual dataset directory and a regular CN file. Read-only reads
may follow links. No automatic copying or dataset conversion is performed.

As with existing publication, there is no stronger fsync/crash durability or
concurrent mutation guarantee. Callers must exclude concurrent dataset/path
changes and concurrent updates (read/merge/write is not a transaction against
other writers). Rename changes the CN inode and may change its mode/ownership to
those of a newly created file. No Parquet or other metadata file is modified.

Region index fingerprints cover Parquet files, metadata, counts and contig sizes;
index summaries contain coordinates, not CN. CN changes do not invalidate these
indexes and do not trigger rebuilding. This makes no guarantee about caches in
external applications.

## Focused checks

`pixi run build`, `pixi run cargo test --locked --profile dev-release copy_numbers`,
and `pixi run python -m unittest discover -s tests -p test_copy_numbers.py -v`.
The Python suite executes the actual sibling CPhasing `_load_cn_info` method AST
without importing optional CPhasing dependencies; it skips this check if that
source checkout is unavailable. It does not claim an end-to-end CPhasing run.

## Validation record (2026-09-09)

Commands ran in pqsio using its Pixi environment:

- `pixi run build`: passed, locked dependencies, dev-release profile.
- `pixi run cargo test --locked --profile dev-release -j 4`: 33 Rust tests passed.
- `pixi run test-copy-numbers`: 9 tests passed, including the actual archived
  v0.0.1 native library, actual CPhasing Python loader method, C ABI invalid input,
  links/replacement, unchanged core/index files and subset/merge behavior.
- `pixi run python -m unittest discover -s tests -p 'test_*.py' -v`: 107 tests
  ran, 105 passed; two existing tests requiring a full `cphasing.pqs` import failed
  with `ModuleNotFoundError: No module named 'cphasing'`. This broad run preceded
  addition of the ninth CN test; the final focused nine-test suite passed.
  C/C++ native smoke tests and the existing cphasing-rs writer compatibility test
  passed in that broad run. No full CPhasing Python pipeline compatibility claim.
- `pixi run lint`: strict Clippy (`--all-targets -- -D warnings`) passed.
- `git diff --check`: passed.

An initial custom Cargo invocation from the parent directory failed to locate
Cargo.toml; rerunning from pqsio produced the passing Rust result above.
No new/upgraded dependencies, large datasets, or release builds were needed for
this feature. It is released as v0.0.12. No 32-bit/aarch64 platform or
crash/concurrent-mutation validation was performed. New CN construction targets
synchronous Writer only.

Changed files: `src/copy_numbers.rs`, `src/lib.rs`, `src/ffi.rs`,
`src/inspection.rs`, `src/merge.rs`, `src/subset.rs`, `include/pqsio.h`,
`python/pqsio/copy_numbers.py`, `python/pqsio/__init__.py`,
`tests/test_copy_numbers.py`, `tests/test_merge.py`, `tests/test_subset.py`,
`pixi.toml`, `README.md`, `docs/copy-numbers.md`, `docs/inspection.md`,
`docs/merge.md`, and `docs/subset.md`.
