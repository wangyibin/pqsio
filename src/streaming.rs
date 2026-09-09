//! Additive row-group streaming reader. Legacy Reader behavior is unchanged.
use crate::*;
use std::collections::VecDeque;
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

pub struct StreamingReader {
    pub(crate) source: Reader,
    options: ReadOptions,
    min_mapq: u8,
    decoder: Option<(File, ParquetReader<File>, usize)>,
    pairs: std::vec::IntoIter<Pair>,
    alignments: std::vec::IntoIter<Alignment>,
    previous_raw: Option<u64>,
    mapped_id: u64,
    pending: Vec<Alignment>,
    ready: VecDeque<Alignment>,
    eof: bool,
    pub(crate) failed: bool,
}
impl StreamingReader {
    pub fn open(path: impl AsRef<Path>, min_mapq: u8, options: ReadOptions) -> Result<Self> {
        ensure!(
            options.batch_rows > 0 && options.batch_rows <= u32::MAX as usize,
            "batch_rows must be in 1..=4294967295"
        );
        let source = Reader::open(path, 0)?; // Always q0, mapping precedes filtering.
        ensure!(
            source.kind == Kind::Concat
                || (options.boundary == ReadBoundary::Rows && options.concat_filter.is_none()),
            "pairs requires rows boundary and no concat filter option"
        );
        Ok(Self {
            source,
            options,
            min_mapq,
            decoder: None,
            pairs: vec![].into_iter(),
            alignments: vec![].into_iter(),
            previous_raw: None,
            mapped_id: 0,
            pending: vec![],
            ready: VecDeque::new(),
            eof: false,
            failed: false,
        })
    }
    pub fn kind(&self) -> Kind {
        self.source.kind
    }
    pub fn contigs(&self) -> &[Contig] {
        &self.source.contigs
    }
    pub(crate) fn poison(&mut self) {
        self.failed = true;
        self.decoder = None;
        self.pairs = vec![].into_iter();
        self.alignments = vec![].into_iter();
        self.pending = vec![];
        self.ready = VecDeque::new();
    }
    fn decode(&mut self) -> Result<bool> {
        loop {
            if self.decoder.is_none() {
                let Some(path) = self.source.files.next() else {
                    return Ok(false);
                };
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
            if group.num_rows() == 0 {
                continue;
            }
            let mut one = metadata.as_ref().clone();
            one.num_rows = group.num_rows();
            one.row_groups = vec![group];
            let mut decoder =
                ParquetReader::new(file.try_clone()?).read_parallel(ParallelStrategy::None);
            decoder.set_metadata(Arc::new(one));
            match self.source.frame_batch(decoder.finish()?)? {
                Batch::Pairs(rows) => self.pairs = rows.into_iter(),
                Batch::Concat(mut rows) => {
                    for row in &mut rows {
                        let raw = row.read_idx;
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
                            row.read_idx = self.mapped_id;
                        }
                        self.previous_raw = Some(raw);
                    }
                    self.alignments = rows.into_iter();
                }
            }
            return Ok(true);
        }
    }
    fn alignment(&mut self) -> Result<Option<Alignment>> {
        loop {
            if let Some(row) = self.alignments.next() {
                return Ok(Some(row));
            }
            if !self.decode()? {
                return Ok(None);
            }
        }
    }
    fn read(&mut self) -> Result<Vec<Alignment>> {
        loop {
            if self.eof {
                return Ok(vec![]);
            }
            let mut rows = std::mem::take(&mut self.pending);
            loop {
                match self.alignment()? {
                    Some(row) if rows.first().is_some_and(|r| r.read_idx != row.read_idx) => {
                        self.pending.push(row);
                        break;
                    }
                    Some(row) => rows.push(row),
                    None => {
                        self.eof = true;
                        break;
                    }
                }
            }
            if self.options.concat_filter == Some(ConcatFilter::CompleteReads) {
                if !rows.iter().any(|r| r.mapping_quality >= self.min_mapq) {
                    rows.clear();
                }
            } else {
                rows.retain(|r| r.mapping_quality >= self.min_mapq);
            }
            if !rows.is_empty() || self.eof {
                return Ok(rows);
            }
        }
    }
    pub fn next_batch(&mut self) -> Result<Option<Batch>> {
        ensure!(!self.failed, "streaming reader failed; close and reopen it");
        let result = self.next_inner();
        if result.is_err() {
            self.poison();
        }
        result
    }
    fn next_inner(&mut self) -> Result<Option<Batch>> {
        let limit = self.options.batch_rows;
        if self.kind() == Kind::Pairs {
            let mut out = vec![];
            while out.len() < limit {
                match self.pairs.next() {
                    Some(row) if row.mapq >= self.min_mapq => out.push(row),
                    Some(_) => (),
                    None if self.decode()? => (),
                    None => break,
                }
            }
            return Ok((!out.is_empty()).then_some(Batch::Pairs(out)));
        }
        let mut out = vec![];
        // Matching rows needs no read-sized staging.
        if self.options.boundary == ReadBoundary::Rows
            && self.options.concat_filter != Some(ConcatFilter::CompleteReads)
        {
            while out.len() < limit {
                match self.alignment()? {
                    Some(row) if row.mapping_quality >= self.min_mapq => out.push(row),
                    Some(_) => (),
                    None => break,
                }
            }
        } else {
            loop {
                if self.ready.is_empty() {
                    self.ready = self.read()?.into();
                }
                if self.ready.is_empty() {
                    break;
                }
                if self.options.boundary == ReadBoundary::CompleteReads {
                    if !out.is_empty() && self.ready.len() > limit - out.len() {
                        break;
                    }
                    out.extend(self.ready.drain(..));
                } else {
                    let n = self.ready.len().min(limit - out.len());
                    out.extend(self.ready.drain(..n));
                }
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok((!out.is_empty()).then_some(Batch::Concat(out)))
    }
}
