# Quality statistics

```sh
pqsio stats sample.pairs.pqs
pqsio stats sample.concat.pqs --min-mapq 30
pqsio stats sample.pairs.pqs --json > sample.stats.json
```

`stats` always scans q0 once in Rust and prints a terminal table, or the complete
report with `--json`. q1 is a redundant quality partition and is never added to
q0 counts. Use `info` when only the fast metadata/footer summary is needed.
`--progress/--no-progress` follows the usual stderr/terminal behavior.

`--min-mapq` defaults to 0. `scanned_records` counts every q0 record;
`selected_records` counts those meeting the threshold. All distributions and
per-contig counts describe selected records, including records whose stored
`filter_reason` is not `pass`. No additional filter is implicit.

| Report field | Meaning |
| --- | --- |
| `filtered_records` | Scanned minus selected |
| `selected_fraction` | Selected / scanned |
| `mapq_histogram` | 256 entries, indexed by MAPQ |
| `mapq_zero_fraction`, `mapq_ge30_fraction` | Fractions among selected records |
| `per_contig` | Named contig counts, including zeros |
| `per_contig_unit` | `pair_endpoints` for pairs, `alignments` for concat |

## Pairs metrics

- `cis_records` / `trans_records`: same-contig / different-contig pairs.
- `cis_fraction` / `trans_fraction`: fractions among selected pairs.
- `cis_distance_bp`: absolute endpoint distance in five disjoint bins:
  0–999, 1,000–9,999, 10,000–99,999, 100,000–999,999, and >=1,000,000 bp.
- `strand_pairs`: orientation counts in stored endpoint order (`++`, `+-`,
  `-+`, `--` for ordinary strand values).

Both endpoints contribute to contig counts, including twice for a cis pair.
Duplicate records remain counted. Pair record count is not unique biological
read count; this command does not deduplicate or infer PCR duplication.

## Concat metrics

- `scanned_reads`: logical q0 reads before MAPQ filtering.
- `selected_reads`: reads with at least one selected alignment.
- `read_order_histogram`: selected alignments per selected read; JSON keys are
  decimal order values. `mean_read_order` is selected alignments / selected reads.
- `mean_read_length`: stored read length averaged once per selected read.
- `multi_contig_reads`: selected reads whose selected alignments touch >1 contig.
- `filter_reasons`: counts of the exact stored strings, including an empty string.
- `aligned_reference_bases`: sum of selected `end-start` intervals; overlapping
  alignments are not merged and this is not unique coverage or query coverage.
- `mean_identity`: mean of finite stored identity values, without rescaling.
  `nonfinite_identity_records` reports excluded nonfinite values.

Read grouping uses the native reader's logical IDs across batches and shards,
including its existing shard-local-ID mapping. Metrics assume the normal valid
PQS read-order contract. Fractions/means with no denominator are JSON null and
shown as N/A in the table. The table groups MAPQ bins; JSON retains all bins
and the complete per-contig breakdown.

The scan keeps native batches and aggregate counters, not all pairs or read IDs.
Memory includes the largest decoded row group, the contig table, distinct read
orders/filter-reason strings and the current read's contig set. It is not a
fixed-byte-memory guarantee. Counts describe the supplied data; no universal
quality pass/fail threshold is imposed.

## API

```python
report = pqsio.stats("sample.concat.pqs", min_mapq=30).to_dict()
```

Rust: `pqsio::statistics::stats(path, min_mapq: u8) -> Result<metadata::Value>`.
C: `pqsio_stats_json(path, min_mapq, callback, user)` returns 0 or -1, with the
same borrowed length-delimited JSON callback contract as `pqsio_info_json`.
Python only receives the aggregated report; record scanning stays in Rust.
