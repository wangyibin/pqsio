# Structured metadata, inspect and validate

These additive Rust/Python APIs are read-only. They do not migrate metadata,
repair files, modify datasets, or change disk schemas/dependencies. C consumers
can use the JSON callback functions in `pqsio.h`; C++ can use the same C ABI.
No CLI is added.

```python
import pqsio
info = pqsio.inspect('sample.pqs')
print(info.metadata.to_dict())
print(info.to_dict()['observed'])
report = pqsio.validate('sample.pqs', level='full', max_issues=100)
print(report.status)  # valid, invalid, incomplete
print(report.to_dict())
```

```rust
use pqsio::{inspect, validate, ValidationLevel};
# fn example() -> anyhow::Result<()> {
let info = inspect("sample.pqs")?;
let report = validate("sample.pqs", ValidationLevel::Full, 100)?;
println!("{}", info.to_json());
println!("{}", report.to_json());
# Ok(()) }
```

## Metadata

`Metadata::parse` / `Metadata::open` parse a restricted Python-literal dialect
without evaluating code: dictionaries with unique quoted keys, lists, strings,
finite numbers, booleans, None, and allowlisted Polars scalar dtype names
(optionally prefixed by `pl.` or `polars.`). Dtype expressions become strings.
Unknown literal fields are retained. Calls, indexing, comprehensions, arbitrary
names, duplicate keys and trailing expressions are rejected. String escapes
include escaped quotes/backslashes, standard controls, x/u/U Unicode escapes.
Input is limited to 16 MiB and nesting to 64. Other Python expressions are not
supported; this parser is deliberately not a general Python interpreter.

Reader and therefore StreamingReader share this parser. Supported format versions
remain pairs 0.1.0 and concat 0.2.0. Absent read_idx_scope retains the old Reader's
global-ID interpretation; validate flags unsupported explicit scopes. Normal
Reader does not invoke strict validation. Files formerly accepted only because
substring matching accidentally found metadata inside another string are now
rejected. Previously unrecognized executable expressions are also rejected.

`inspect` reads metadata, ordered contigs, optional declared counts, directory
entries and Parquet footers. It does not decode records. Counts are separated
into declared and observed; missing counts are absent, not zero. Coordinates are
derived only for known versions. Unknown versions can still be inspected.
Malformed/unreadable input causes inspect to return an error; validate instead
returns a diagnostic report when possible. The overview deliberately has no
`valid` flag. Unknown sidecars and metadata fields are not deleted or rewritten.

## Validation

- quick: required files/directories, metadata, positive contig lengths, supported
  schema/columns, footer types, and declared versus footer record counts.
- full: the above plus raw q0 and q1 row-group decoding, null/encoding checks,
  coordinates, strands, finite identity, read intervals, read-ID order and
  consistent read length, read counts, q1 MAPQ, and ordered q0/q1 comparison.

Full validation reads original IDs, without MAPQ filtering or ID remapping.
Global read groups can span adjacent files; shard-local groups reset at each
shard. It follows the writer's increasing read-ID and coordinate rules. Counts
of reads are group counts; a decreasing ID is separately diagnosed.

q0/q1 comparison is exact record comparison in existing traversal order, not a
hash-based claim. It ignores global-ID shard boundaries, allowing repartitioned
q1. For shard-local IDs, original shard names must also correspond. If this
ordered comparison differs, the report is **incomplete**, with
Q0_Q1_SEQUENCE_MISMATCH: content may differ, or the same records may have been
reordered. This first implementation does not perform disk-backed unordered
multiset comparison and does not falsely classify a reordering as corruption.
Independent proven violations (for example MAPQ zero in q1 or a count mismatch)
still make the report invalid.

`valid` means the checks for the requested level passed, not certification of
all aspects of PQS. `invalid` means at least one proven violation was found.
`incomplete` means unsupported semantics, unavailable input, or a check that
could not complete, without a proven violation taking precedence. Always examine
checks_completed/checks_skipped; invalid reports can also contain skipped checks.
Missing required components are errors; omitted individual count keys are
warnings. Issues have stable codes, file, optional field and one-based original
shard row. Row-group decoding errors may identify only the file/group context.
The total issue count continues after the stored example cap; decoding failures
abort the remaining full scan and explicitly record skipped checks.

## Resource and compatibility limits

Full validation keeps up to two decoded row groups plus conversion buffers, not
whole datasets or complete oversized reads. Huge row groups/strings can still
use substantial memory. Footer and path storage grows with shard/row-group count.
No arbitrary-layout byte-memory guarantee, parallel scan, remote I/O, content
repair, adversarial Parquet allocation hardening, snapshot isolation against
concurrent writers, or unordered q0/q1 comparison is implemented. Scan completed
datasets that are not being modified. Unknown sidecar contents are not validated.

Python uses only the standard library and copies JSON during the native callback.
`to_dict()` returns an independent deep copy. Older native libraries produce an
explicit capability error for these new APIs; existing APIs remain available.
ABI v1 and existing symbols/structures are unchanged. JSON callbacks must return
zero and cannot retain native bytes or unwind/reenter across the ABI boundary.

## Verification

`pixi run test-inspection` exercises small synthetic datasets, corruption,
metadata safety, raw-ID/group checks, q1 differences, argument handling and the
actual C callback bridge. `pixi run test-rust` includes the direct Rust APIs.
`pixi run test` includes existing reader/writer, columnar, parallel and streaming
regressions. No whole-genome benchmark is part of these checks.

## Implementation and validation record (2026-09-09)

Changed files:

- `src/metadata.rs`: restricted parser and structured literal values.
- `src/inspection.rs`: footer overview, diagnostics and raw full scan.
- `src/lib.rs`: public exports and shared Reader metadata entry.
- `src/ffi.rs`, `include/pqsio.h`: additive JSON callback ABI.
- `python/pqsio/inspection.py`, `python/pqsio/__init__.py`: Python public API.
- `tests/inspection.rs`, `tests/test_inspection.py`: Rust and Python/native tests.
- `pixi.toml`: focused test task included in aggregate tests.
- `README.md`, `CHANGELOG.md`, `docs/inspection.md`: documentation.

Executed `pixi run test` successfully (existing Rust, Python, C/C++, columnar,
parallel and streaming regressions included). After additional inspection edge
cases, `pixi run test-inspection` passed 11 tests and `pixi run test-rust` passed
20 tests. Strict `pixi run lint` and `git diff --check` passed. Builds used
Pixi/dev-release. Initial compilation exposed Arrow-footer schema conversion and
borrow-lifetime errors, corrected before testing. Fixture-only Python Polars
emitted a categorical re-encoding warning when combining q1 shards; tests passed.

No whole-genome run, throughput/RSS measurement, external CPhasing integration
suite, alternate-platform build, or general unordered comparison was performed.
No dependencies or lockfiles were changed. Released as v0.0.8.

Optional `cn.info` is included as structured `copy_numbers` inspection data and
checked in quick mode. Local malformed/unreadable CN diagnostics do not prevent
other core checks. See [CN fields, codes and limits](copy-numbers.md).
