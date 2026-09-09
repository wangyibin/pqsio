//! Additive row-group streaming reader. Legacy Reader behavior is unchanged.
use crate::*;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadBoundary {
    Rows,
    CompleteReads,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcatFilter {
    MatchingAlignments,
    CompleteReads,
}
#[derive(Clone, Copy, Debug)]
pub struct ReadOptions {
    pub batch_rows: usize,
    pub boundary: ReadBoundary,
    /// None selects matching alignments for concat; pairs requires None.
    pub concat_filter: Option<ConcatFilter>,
}
impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            batch_rows: 65_536,
            boundary: ReadBoundary::Rows,
            concat_filter: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReadOrigin {
    pub logical_id: u64,
    pub raw_id: u64,
    pub shards: Vec<PathBuf>,
}

pub struct StreamingReader {
    pub(crate) source: Reader,
    options: ReadOptions,
    pub(crate) query: Option<crate::query::QueryState>,
    shard_ordinal: usize,
    capture_origin: bool,
    raw_ids: Vec<u64>,
    current_shard: PathBuf,
    ready_origin: Option<ReadOrigin>,
    pub(crate) origins: Vec<ReadOrigin>,
    min_mapq: u8,
    decoder: Option<(File, ParquetReader<File>, usize)>,
    pairs: PairColumns,
    alignments: ConcatColumns,
    position: usize,
    previous_raw: Option<u64>,
    mapped_id: u64,
    ready: ConcatColumns,
    ready_position: usize,
    eof: bool,
    pub(crate) failed: bool,
}
impl StreamingReader {
    pub fn open(path: impl AsRef<Path>, min_mapq: u8, options: ReadOptions) -> Result<Self> {
        // Complete reads need low-MAPQ alignments from q0. Otherwise reuse
        // Reader's q1 selection, including its shard-local q0 mapping fallback.
        let source_mapq = if options.concat_filter == Some(ConcatFilter::CompleteReads) {
            0
        } else {
            min_mapq
        };
        Self::from_source(Reader::open(path, source_mapq)?, min_mapq, options)
    }
    // QueryReader can supply its already-selected partition.
    pub(crate) fn from_source(source: Reader, min_mapq: u8, options: ReadOptions) -> Result<Self> {
        ensure!(
            options.batch_rows > 0 && options.batch_rows <= u32::MAX as usize,
            "batch_rows must be in 1..=4294967295"
        );
        ensure!(
            source.kind == Kind::Concat
                || (options.boundary == ReadBoundary::Rows && options.concat_filter.is_none()),
            "pairs requires rows boundary and no concat filter option"
        );
        Ok(Self {
            source,
            options,
            query: None,
            shard_ordinal: 0,
            capture_origin: false,
            raw_ids: vec![],
            current_shard: PathBuf::new(),
            ready_origin: None,
            origins: vec![],
            min_mapq,
            decoder: None,
            pairs: PairColumns::default(),
            alignments: ConcatColumns::default(),
            position: 0,
            previous_raw: None,
            mapped_id: 0,
            ready: ConcatColumns::default(),
            ready_position: 0,
            eof: false,
            failed: false,
        })
    }
    pub(crate) fn capture_origins(&mut self) {
        self.capture_origin = true;
    }
    pub fn kind(&self) -> Kind {
        self.source.kind
    }
    pub fn contigs(&self) -> &[Contig] {
        &self.source.contigs
    }
    pub(crate) fn poison(&mut self) {
        self.failed = true;
        if let Some(query) = &mut self.query {
            query.cursor = None;
            query.stats.complete = false;
        }
        self.decoder = None;
        self.pairs = PairColumns::default();
        self.alignments = ConcatColumns::default();
        self.ready = ConcatColumns::default();
        self.position = 0;
        self.ready_position = 0;
    }
    fn decode(&mut self) -> Result<bool> {
        if self.eof {
            return Ok(false);
        }
        // Release the exhausted group before allocating the next one.
        self.pairs = PairColumns::default();
        self.alignments = ConcatColumns::default();
        self.position = 0;
        loop {
            if self.decoder.is_none() {
                let Some(path) = self.source.files.next() else {
                    self.eof = true;
                    return Ok(false);
                };
                self.current_shard = path.clone();
                self.shard_ordinal += 1;
                let file = File::open(path)?;
                self.decoder = Some((file.try_clone()?, ParquetReader::new(file), 0));
                if self.source.shard_scoped {
                    self.previous_raw = None;
                }
            }
            let (file, reader, index) = self.decoder.as_mut().unwrap();
            let metadata = reader.get_metadata()?;
            if *index == metadata.row_groups.len() {
                self.decoder = None;
                continue;
            }
            // Preserve absolute column byte offsets, but expose exactly one row
            // group. No prefix slicing/redecoding, and no whole-shard DataFrame.
            let group = metadata.row_groups[*index].clone();
            *index += 1;
            if let Some(query) = &mut self.query {
                let candidate = if let Some(cursor) = &mut query.cursor {
                    cursor.candidate(
                        self.shard_ordinal - 1,
                        *index - 1,
                        group.num_rows(),
                        &query.predicate,
                    )?
                } else {
                    !query.predicate.is_empty()
                };
                if !candidate {
                    query.stats.skipped_row_groups += 1;
                    continue;
                }
                query.stats.candidate_row_groups += 1;
            }
            if group.num_rows() == 0 {
                continue;
            }
            let mut one = metadata.as_ref().clone();
            one.num_rows = group.num_rows();
            one.row_groups = vec![group];
            let mut decoder =
                ParquetReader::new(file.try_clone()?).read_parallel(ParallelStrategy::None);
            decoder.set_metadata(Arc::new(one));
            let frame = decoder.finish()?;
            if let Some(query) = &mut self.query {
                query.stats.decoded_row_groups += 1;
                query.stats.decoded_rows += frame.height() as u64;
            }
            match self.source.frame_columns(frame)? {
                ColumnBatch::Pairs(columns) => self.pairs = columns,
                ColumnBatch::Concat(mut columns) => {
                    if self.capture_origin {
                        self.raw_ids.clone_from(&columns.read_idx);
                    }
                    for id in &mut columns.read_idx {
                        let raw = *id;
                        if let Some(previous) = self.previous_raw {
                            ensure!(raw >= previous, "concat read IDs must be contiguous and increasing within their ID scope");
                        }
                        if self.source.shard_scoped {
                            if self.previous_raw != Some(raw) {
                                self.mapped_id = self.source.next_read_id;
                                self.source.next_read_id = self
                                    .mapped_id
                                    .checked_add(1)
                                    .context("logical read ID overflow")?;
                            }
                            *id = self.mapped_id;
                        }
                        self.previous_raw = Some(raw);
                    }
                    self.alignments = columns;
                }
            }
            return Ok(true);
        }
    }
    fn ensure_alignment(&mut self) -> Result<bool> {
        while self.position == self.alignments.read_idx.len() {
            if !self.decode()? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn read(&mut self) -> Result<ConcatColumns> {
        loop {
            let mut out = ConcatColumns::default();
            self.ready_origin = None;
            while self.ensure_alignment()? {
                let start = self.position;
                let id = self.alignments.read_idx[start];
                if out.read_idx.first().is_some_and(|&previous| previous != id) {
                    break;
                }
                let end =
                    start + self.alignments.read_idx[start..].partition_point(|&next| next == id);
                if self.capture_origin {
                    let origin = self.ready_origin.get_or_insert_with(|| ReadOrigin {
                        logical_id: id,
                        raw_id: self.raw_ids[start],
                        shards: vec![],
                    });
                    if origin.shards.last() != Some(&self.current_shard) {
                        origin.shards.push(self.current_shard.clone());
                    }
                }
                out.append(self.alignments.as_view(), start, end);
                self.position = end;
            }
            if self.options.concat_filter == Some(ConcatFilter::CompleteReads) {
                if !out.mapping_quality.iter().enumerate().any(|(i, &q)| {
                    q >= self.min_mapq
                        && self
                            .query
                            .as_ref()
                            .is_none_or(|s| s.predicate.alignment(&out, i))
                }) {
                    out = ConcatColumns::default();
                }
            } else if self.min_mapq > 0 || self.query.is_some() {
                // With no predicate every alignment is retained; reuse the
                // assembled read instead of allocating and copying all columns.
                let mut filtered = ConcatColumns::default();
                let mut start = 0;
                while start < out.read_idx.len() {
                    let end = matching_run(
                        &out.mapping_quality,
                        &mut start,
                        usize::MAX,
                        self.min_mapq,
                        |i| {
                            self.query
                                .as_ref()
                                .is_none_or(|s| s.predicate.alignment(&out, i))
                        },
                    );
                    if start < end {
                        filtered.append(out.as_view(), start, end);
                    }
                    start = end;
                }
                out = filtered;
            }
            if !out.read_idx.is_empty() || self.eof {
                return Ok(out);
            }
        }
    }
    /// Legacy row output: materialize rows only after shared columnar batching.
    pub fn next_batch(&mut self) -> Result<Option<Batch>> {
        let result = self
            .next_columns()
            .and_then(|b| b.map(ColumnBatch::into_rows).transpose());
        if result.is_err() {
            self.poison();
        }
        result
    }
    /// Owned typed buffers, independent of this reader's lifetime. Uses the same
    /// cursor, boundaries, filtering and terminal-error state as next_batch.
    pub fn next_columns(&mut self) -> Result<Option<ColumnBatch>> {
        ensure!(!self.failed, "streaming reader failed; close and reopen it");
        let result = self.next_inner().with_context(|| {
            format!("streaming shard {}", self.current_shard.display())
        });
        if let (Ok(batch), Some(query)) = (&result, &mut self.query) {
            if let Some(batch) = batch {
                query.stats.returned_rows += match batch {
                    ColumnBatch::Pairs(c) => c.pos1.len(),
                    ColumnBatch::Concat(c) => c.start.len(),
                } as u64;
            } else {
                query.stats.complete = true;
            }
        }
        if result.is_err() {
            self.poison();
        }
        result
    }
    fn next_inner(&mut self) -> Result<Option<ColumnBatch>> {
        self.origins.clear();
        let limit = self.options.batch_rows;
        if self.kind() == Kind::Pairs {
            let mut out = PairColumns::default();
            while out.pos1.len() < limit {
                if self.position == self.pairs.pos1.len() && !self.decode()? {
                    break;
                }
                let end = matching_run(
                    &self.pairs.mapq,
                    &mut self.position,
                    limit - out.pos1.len(),
                    self.min_mapq,
                    |i| {
                        self.query
                            .as_ref()
                            .is_none_or(|s| s.predicate.pair(&self.pairs, i))
                    },
                );
                if self.position < end {
                    out.append(self.pairs.as_view(), self.position, end);
                }
                self.position = end;
            }
            return Ok((!out.pos1.is_empty()).then_some(ColumnBatch::Pairs(out)));
        }
        let mut out = ConcatColumns::default();
        if self.options.boundary == ReadBoundary::Rows
            && self.options.concat_filter != Some(ConcatFilter::CompleteReads)
        {
            while out.read_idx.len() < limit && self.ensure_alignment()? {
                let end = matching_run(
                    &self.alignments.mapping_quality,
                    &mut self.position,
                    limit - out.read_idx.len(),
                    self.min_mapq,
                    |i| {
                        self.query
                            .as_ref()
                            .is_none_or(|s| s.predicate.alignment(&self.alignments, i))
                    },
                );
                if self.position < end {
                    out.append(self.alignments.as_view(), self.position, end);
                }
                self.position = end;
            }
        } else {
            loop {
                if self.ready_position == self.ready.read_idx.len() {
                    self.ready = ConcatColumns::default();
                    self.ready = self.read()?;
                    self.ready_position = 0;
                }
                if self.ready.read_idx.is_empty() {
                    break;
                }
                let remaining = self.ready.read_idx.len() - self.ready_position;
                let n = if self.options.boundary == ReadBoundary::CompleteReads {
                    if !out.read_idx.is_empty() && remaining > limit - out.read_idx.len() {
                        break;
                    }
                    remaining
                } else {
                    remaining.min(limit - out.read_idx.len())
                };
                if self.capture_origin {
                    if let Some(origin) = self.ready_origin.take() {
                        self.origins.push(origin);
                    }
                }
                out.append(
                    self.ready.as_view(),
                    self.ready_position,
                    self.ready_position + n,
                );
                self.ready_position += n;
                if out.read_idx.len() >= limit {
                    break;
                }
            }
        }
        Ok((!out.read_idx.is_empty()).then_some(ColumnBatch::Concat(out)))
    }
}

// Skip unmatched values, then take one contiguous matching span. No row objects.
fn matching_run(
    quality: &[u8],
    start: &mut usize,
    limit: usize,
    min_mapq: u8,
    matches: impl Fn(usize) -> bool,
) -> usize {
    while *start < quality.len() && (quality[*start] < min_mapq || !matches(*start)) {
        *start += 1;
    }
    let mut end = *start;
    while end < quality.len() && end - *start < limit && quality[end] >= min_mapq && matches(end) {
        end += 1;
    }
    end
}
