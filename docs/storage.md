# Storage and compatibility

For a user-facing introduction to directory layout, fields and coordinates,
start with [PQS format](pqs-format.md).

!!! warning "External reader compatibility"

    During v0.2.1 validation, the external CPhasing reader with PyArrow 10.0.1
    rejected generated concat Parquet footers with `Unrecognized type:24`, for
    both synchronous and parallel output. Native pqsio round trips passed.
    The minimum compatible PyArrow version has not been established; check
    your downstream reader before adopting this release for that workflow.

- Both datasets have `_contigsizes`, `_metadata`, `_metadata_counts`, `_readme`,
  `q0/` and `q1/`. q0 contains **all** records; q1 contains MAPQ >= 1, not a
  disjoint complement. Concat uses `mapping_quality`; pairs uses `mapq`.
- pairs positions are **1-based**. concat reference/query intervals are
  **0-based, half-open**. No implicit coordinate conversion is performed.
- pairs positions use UInt32 when contig lengths permit, otherwise UInt64.
  concat reference positions use UInt64, compatible with the existing format;
  read length and read offsets remain UInt32.
- The metadata retains CPhasing's Python-dictionary representation, including
  Polars dtype names. The Rust reader does not execute the metadata as code.
- Writer calls validate before accepting the batch/read. A storage error marks
  the writer failed. Data is staged in `<output>.partial`, and a successful
  `finish` publishes the result. Existing outputs or staging directories are
  rejected. Drop/close aborts unfinished output. A process crash can leave a
  `.partial` directory; investigate/remove it before retrying. Publication uses
  same-filesystem rename, with a brief empty destination reservation to avoid
  replacing a concurrent creator; readers only recognize completed metadata.
  This is not an fsync-based crash-durability guarantee.
- Empty datasets are supported and have no Parquet shards. A concat read larger
  than the target shard size occupies one oversized shard. Other complete reads
  are packed without crossing shard boundaries.
- min_mapq > 0 yields only retained alignments, not the original full read.
  When reading existing concat metadata with `read_idx_scope='shard'`, the
  reader scans q0 and assigns deterministic increasing global IDs before
  filtering, keeping separate shards' equal IDs separate. Global IDs are kept
  unchanged. Read order follows numeric shard names, then other names.
- This version preserves the core records/schema. It does not automatically
  copy optional application files such as `cn.info`; it is not a general
  lossless clone operation for all dataset sidecars.
