//! PQS storage. Coordinates are passed through unchanged; pairs uses 1-based
//! positions, concat uses 0-based half-open reference and read intervals.
use anyhow::{bail, ensure, Context, Result};
use polars::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    path::{Path, PathBuf},
};
pub mod copy_numbers;
pub use copy_numbers::{CopyNumbers, read_copy_numbers, set_copy_numbers, update_copy_numbers};
pub mod subset;
pub use subset::{subset, SubsetOptions, SubsetResult, ReadIds};
pub mod merge;
pub use merge::{merge, MergeOptions, MergeResult, MergeSource};
pub mod convert;
pub use convert::{convert, ConvertOptions, ConvertResult};
pub mod import;
pub use import::{import_alignments, ImportOptions, ImportResult};
pub mod io;
pub mod progress;
pub mod presentation;
pub mod statistics;
pub mod cool;
pub use cool::{pairs2cool, CoolOptions, CoolResult};
pub mod metadata;
pub mod inspection;
pub use metadata::Metadata;
pub use inspection::{inspect, validate, Inspection, ValidationLevel, ValidationReport};
pub mod columns;
#[cfg(test)]
mod frame_columns_bench;
pub mod ffi;
pub use columns::{ColumnBatch, ConcatColumns, ConcatColumnsView, PairColumns, PairColumnsView};
pub mod query;
pub use query::{build_index, build_index_for_quality, IndexQuality, IndexMode, PairsMode, QueryOptions, QueryReader, QueryStats, Region};
pub mod streaming;
pub use streaming::{ConcatFilter, ReadBoundary, ReadOptions, StreamingReader};
pub mod parallel;
pub use parallel::{ParallelOptions, ParallelWriter, Producer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pairs,
    Concat,
}
#[derive(Clone, Debug)]
pub struct Contig {
    pub name: String,
    pub length: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Pair {
    pub read_id: String,
    pub chrom1: u32,
    pub pos1: u64,
    pub chrom2: u32,
    pub pos2: u64,
    pub strand1: u8,
    pub strand2: u8,
    pub mapq: u8,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Alignment {
    pub read_idx: u64,
    pub read_length: u32,
    pub read_start: u32,
    pub read_end: u32,
    pub strand: u8,
    pub chrom: u32,
    pub start: u64,
    pub end: u64,
    pub mapping_quality: u8,
    pub identity: f32,
    pub filter_reason: String,
}
#[derive(Debug, PartialEq)]
pub enum Batch {
    Pairs(Vec<Pair>),
    Concat(Vec<Alignment>),
}
#[derive(Default, Clone, Copy, Debug)]
pub struct Counts {
    pub q0_records: u64,
    pub q1_records: u64,
    pub q0_concats: u64,
    pub q1_concats: u64,
}

/// Synchronous bounded writer. Concat submissions contain complete reads,
/// with strictly increasing global read IDs. A single oversized read is
/// allowed to exceed the shard target, but is never split.
pub struct Writer {
    kind: Kind,
    contigs: Vec<Contig>,
    target: PathBuf,
    staging: PathBuf,
    chunk_size: usize,
    columns: Option<ColumnBatch>,
    pairs: Vec<Pair>,
    concat: Vec<Alignment>,
    last_read: Option<u64>,
    shard: usize,
    shard_concats: [u64; 2],
    counts: Counts,
    finished: bool,
    failed: bool,
    executor: Option<parallel::Executor>,
}
impl Writer {
    pub fn create(
        path: impl AsRef<Path>,
        kind: Kind,
        contigs: Vec<Contig>,
        chunk_size: usize,
    ) -> Result<Self> {
        ensure!(chunk_size > 0, "chunk_size must be positive");
        ensure!(!contigs.is_empty(), "at least one contig is required");
        let mut names = HashSet::new();
        for c in &contigs {
            ensure!(
                !c.name.is_empty() && !c.name.chars().any(char::is_whitespace),
                "invalid contig name"
            );
            ensure!(
                c.length > 0 && names.insert(c.name.clone()),
                "duplicate contig or zero length: {}",
                c.name
            );
        }
        let target = path.as_ref().to_path_buf();
        ensure!(
            !target.exists(),
            "output already exists: {}",
            target.display()
        );
        let mut name = target.as_os_str().to_os_string();
        name.push(".partial");
        let staging = PathBuf::from(name);
        fs::create_dir(&staging).context("cannot reserve output .partial directory")?;
        let result = (|| {
            fs::create_dir(staging.join("q0"))?;
            fs::create_dir(staging.join("q1"))?;
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
        Ok(Self {
            kind,
            contigs,
            target,
            staging,
            chunk_size,
            columns: None,
            pairs: vec![],
            concat: vec![],
            last_read: None,
            shard: 0,
            shard_concats: [0, 0],
            counts: Counts::default(),
            finished: false,
            failed: false,
            executor: None,
        })
    }
    fn active(&self) -> Result<()> {
        ensure!(
            !self.finished && !self.failed,
            "writer is finished or failed"
        );
        Ok(())
    }
    fn position(&self, chrom: u32, pos: u64) -> Result<()> {
        let c = self
            .contigs
            .get(chrom as usize)
            .context("contig ID is out of range")?;
        ensure!(pos <= c.length, "position exceeds contig {} length", c.name);
        Ok(())
    }
    fn validate_pairs(&self, rows: &[Pair]) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Pairs, "requires pairs writer");
        for r in rows {
            self.pair_fields(
                r.chrom1, r.pos1, r.chrom2, r.pos2, r.strand1, r.strand2,
            )?;
        }
        Ok(())
    }
    pub fn write_pairs(&mut self, rows: &[Pair]) -> Result<()> {
        self.validate_pairs(rows)?;
        if !rows.is_empty() {
            self.flush_columns()?;
        }
        for part in rows.chunks(self.chunk_size) {
            // Extend up to the boundary without cloning an intermediate batch.
            let mut remaining = part;
            while !remaining.is_empty() {
                let n = remaining.len().min(self.chunk_size - self.pairs.len());
                self.pairs.extend_from_slice(&remaining[..n]);
                remaining = &remaining[n..];
                if self.pairs.len() == self.chunk_size {
                    self.flush()?;
                }
            }
        }
        Ok(())
    }
    /// Transfer records and their strings into the writer without cloning them.
    pub fn write_pairs_owned(&mut self, rows: Vec<Pair>) -> Result<()> {
        self.validate_pairs(&rows)?;
        if !rows.is_empty() {
            self.flush_columns()?;
        }
        let mut rows = rows.into_iter();
        while rows.len() > 0 {
            let n = rows.len().min(self.chunk_size - self.pairs.len());
            self.pairs.extend(rows.by_ref().take(n));
            if self.pairs.len() == self.chunk_size {
                self.flush()?;
            }
        }
        Ok(())
    }
    fn validate_read(&self, rows: &[Alignment], previous: Option<u64>) -> Result<bool> {
        let first = rows
            .first()
            .context("a complete read must contain at least one alignment")?;
        ensure!(
            previous.is_none_or(|id| first.read_idx > id),
            "read IDs must be strictly increasing; submit each complete read once"
        );
        let mut has_q1 = false;
        for r in rows {
            ensure!(
                r.read_idx == first.read_idx && r.read_length == first.read_length,
                "read ID and length must agree within a complete read"
            );
            self.alignment_fields(
                (r.read_length, r.read_start, r.read_end),
                (r.chrom, r.start, r.end), r.strand, r.identity,
            )?;
            has_q1 |= r.mapping_quality > 0;
        }
        Ok(has_q1)
    }
    fn start_read(&mut self, len: usize, id: u64, has_q1: bool) -> Result<()> {
        if !self.concat.is_empty() && self.concat.len().saturating_add(len) > self.chunk_size {
            self.flush()?;
        }
        self.last_read = Some(id);
        self.shard_concats[0] += 1;
        self.shard_concats[1] += u64::from(has_q1);
        Ok(())
    }
    fn finish_read(&mut self) -> Result<()> {
        if self.concat.len() >= self.chunk_size {
            self.flush()?;
        }
        Ok(())
    }
    pub fn write_read(&mut self, rows: &[Alignment]) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Concat, "requires concat writer");
        let has_q1 = self.validate_read(rows, self.last_read)?;
        self.flush_columns()?;
        self.start_read(rows.len(), rows[0].read_idx, has_q1)?;
        self.concat.extend_from_slice(rows);
        self.finish_read()
    }
    /// Offsets start at zero and end at rows.len(); each interval is one
    /// nonempty complete read. The whole batch is validated before acceptance.
    fn validate_reads(&self, rows: &[Alignment], offsets: &[usize]) -> Result<Vec<bool>> {
        self.active()?;
        ensure!(self.kind == Kind::Concat, "requires concat writer");
        ensure!(
            offsets.first() == Some(&0) && offsets.last() == Some(&rows.len()),
            "read offsets must start at 0 and end at the number of alignments"
        );
        ensure!(
            offsets
                .windows(2)
                .all(|w| w[0] < w[1] && w[1] <= rows.len()),
            "read offsets must be strictly increasing and within the batch"
        );
        let mut previous = self.last_read;
        let mut qualities = Vec::with_capacity(offsets.len() - 1);
        for window in offsets.windows(2) {
            let read = &rows[window[0]..window[1]];
            qualities.push(self.validate_read(read, previous)?);
            previous = Some(read[0].read_idx);
        }
        Ok(qualities)
    }
    pub fn write_reads(&mut self, rows: &[Alignment], offsets: &[usize]) -> Result<()> {
        let qualities = self.validate_reads(rows, offsets)?;
        if !rows.is_empty() {
            self.flush_columns()?;
        }
        for (window, has_q1) in offsets.windows(2).zip(qualities) {
            let read = &rows[window[0]..window[1]];
            self.start_read(read.len(), read[0].read_idx, has_q1)?;
            self.concat.extend_from_slice(read);
            self.finish_read()?;
        }
        Ok(())
    }
    /// Owned counterpart of write_reads, used by the C/Python adapters to
    /// avoid cloning all alignment strings a second time.
    pub fn write_reads_owned(&mut self, rows: Vec<Alignment>, offsets: &[usize]) -> Result<()> {
        let qualities = self.validate_reads(&rows, offsets)?;
        if !rows.is_empty() {
            self.flush_columns()?;
        }
        let mut rows = rows.into_iter().peekable();
        for (window, has_q1) in offsets.windows(2).zip(qualities) {
            let n = window[1] - window[0];
            let id = rows.peek().expect("validated nonempty read").read_idx;
            self.start_read(n, id, has_q1)?;
            self.concat.extend(rows.by_ref().take(n));
            self.finish_read()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.pairs.is_empty() && self.concat.is_empty() {
            return Ok(());
        }
        let result = self.flush_inner();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn flush_inner(&mut self) -> Result<()> {
        let job = parallel::Shard {
            columns: None,
            kind: self.kind,
            contigs: self.contigs.clone(),
            staging: self.staging.clone(),
            index: self.shard,
            pairs: std::mem::take(&mut self.pairs),
            concat: std::mem::take(&mut self.concat),
            concats: self.shard_concats,
        };
        if let Some(executor) = &mut self.executor {
            executor.submit(job, &mut self.counts)?;
        } else {
            parallel::add_counts(&mut self.counts, job.run()?);
            // Preserve synchronous buffer reuse across shards.
            self.pairs = job.pairs;
            self.concat = job.concat;
        }
        self.shard_concats = [0, 0];
        self.pairs.clear();
        self.concat.clear();
        self.shard += 1;
        Ok(())
    }
    // Only merge's fixed sidecars can be created; Writer retains cleanup ownership.
    pub(crate) fn merge_sidecar(&mut self, reads: bool) -> Result<File> {
        self.active()?;
        let name = if reads { "_merge_reads.jsonl" } else { "_merge_sources.jsonl" };
        let result = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.staging.join(name));
        if result.is_err() {
            self.failed = true;
        }
        Ok(result?)
    }
    pub fn finish(&mut self) -> Result<Counts> {
        self.active()?;
        let result = self.finish_inner();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn finish_inner(&mut self) -> Result<Counts> {
        self.flush()?;
        self.flush_columns()?;
        if let Some(executor) = &mut self.executor {
            executor.drain(&mut self.counts)?;
        }
        let sizes = self
            .contigs
            .iter()
            .map(|c| format!("{}\t{}\n", c.name, c.length))
            .collect::<String>();
        fs::write(self.staging.join("_contigsizes"), sizes)?;
        fs::write(
            self.staging.join("_metadata"),
            metadata(self.kind, self.chunk_size, &self.contigs),
        )?;
        let c = self.counts;
        let mut counts = format!(
            "q0_records\t{}\nq1_records\t{}\n",
            c.q0_records, c.q1_records
        );
        if self.kind == Kind::Concat {
            counts.push_str(&format!(
                "q0_concats\t{}\nq1_concats\t{}\n",
                c.q0_concats, c.q1_concats
            ));
        }
        fs::write(self.staging.join("_metadata_counts"), counts)?;
        fs::write(
            self.staging.join("_readme"),
            "PQS: q0 contains all records; q1 contains records with MAPQ >= 1.\n",
        )?;
        // Reserve the destination so a concurrent creator is never overwritten.
        fs::create_dir(&self.target).context("output appeared before publication")?;
        if let Err(e) = fs::rename(&self.staging, &self.target) {
            let _ = fs::remove_dir(&self.target);
            return Err(e.into());
        }
        self.finished = true;
        Ok(c)
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        // Join file writers before removing their staging directory.
        self.executor.take();
        if !self.finished {
            let _ = fs::remove_dir_all(&self.staging);
        }
    }
}
fn strand_ok(s: u8) -> bool {
    s == b'+' || s == b'-'
}
fn pos_dtype(contigs: &[Contig]) -> DataType {
    if contigs.iter().all(|c| c.length <= u32::MAX as u64) {
        DataType::UInt32
    } else {
        DataType::UInt64
    }
}
macro_rules! col {
    ($rows:expr, $name:expr, $value:expr) => {
        Series::new($name.into(), $rows.iter().map($value).collect::<Vec<_>>()).into()
    };
}
fn categorize(df: &mut DataFrame, names: &[&str]) -> Result<()> {
    for name in names {
        let s = df
            .column(name)?
            .as_materialized_series()
            .cast(&DataType::Categorical(None, CategoricalOrdering::Physical))?;
        df.replace(name, s)?;
    }
    Ok(())
}
// Centralize disk dtype/category choices for both input representations.
fn storage_frame(kind: Kind, columns: Vec<Column>, contigs: &[Contig]) -> Result<DataFrame> {
    let mut df = DataFrame::new(columns)?;
    if kind == Kind::Pairs {
        for name in ["pos1", "pos2"] {
            let s = df.column(name)?.as_materialized_series().cast(&pos_dtype(contigs))?;
            df.replace(name, s)?;
        }
        categorize(&mut df, &["chrom1", "chrom2", "strand1", "strand2"])?;
    } else {
        categorize(&mut df, &["strand", "chrom", "filter_reason"])?;
    }
    Ok(df)
}
fn pair_frame(rows: &[Pair], contigs: &[Contig]) -> Result<DataFrame> {
    storage_frame(Kind::Pairs, vec![
        col!(rows, "read_idx", |r| r.read_id.as_str()),
        col!(rows, "chrom1", |r| contigs[r.chrom1 as usize].name.as_str()),
        col!(rows, "pos1", |r| r.pos1),
        col!(rows, "chrom2", |r| contigs[r.chrom2 as usize].name.as_str()),
        col!(rows, "pos2", |r| r.pos2),
        col!(rows, "strand1", |r| if r.strand1 == b'+' {
            "+"
        } else {
            "-"
        }),
        col!(rows, "strand2", |r| if r.strand2 == b'+' {
            "+"
        } else {
            "-"
        }),
        col!(rows, "mapq", |r| r.mapq),
    ], contigs)
}
fn concat_frame(rows: &[Alignment], contigs: &[Contig]) -> Result<DataFrame> {
    storage_frame(Kind::Concat, vec![
        col!(rows, "read_idx", |r| r.read_idx),
        col!(rows, "read_length", |r| r.read_length),
        col!(rows, "read_start", |r| r.read_start),
        col!(rows, "read_end", |r| r.read_end),
        col!(rows, "strand", |r| if r.strand == b'+' { "+" } else { "-" }),
        col!(rows, "chrom", |r| contigs[r.chrom as usize].name.as_str()),
        col!(rows, "start", |r| r.start),
        col!(rows, "end", |r| r.end),
        col!(rows, "mapping_quality", |r| r.mapping_quality),
        col!(rows, "identity", |r| r.identity),
        col!(rows, "filter_reason", |r| r.filter_reason.as_str()),
    ], contigs)
}

/// Reads one existing Parquet shard at a time. Does not evaluate Python metadata.
/// q1 is a filtered view: zero-MAPQ alignments are intentionally absent.
pub struct Reader {
    pub kind: Kind,
    pub contigs: Vec<Contig>,
    files: std::vec::IntoIter<PathBuf>,
    min_mapq: u8,
    contig_ids: HashMap<String, u32>,
    shard_scoped: bool,
    next_read_id: u64,
}
impl Reader {
    pub fn open(path: impl AsRef<Path>, min_mapq: u8) -> Result<Self> {
        Self::open_inner(path.as_ref(), min_mapq, None)
    }
    pub(crate) fn open_partition(path: &Path, q1: bool) -> Result<Self> {
        Self::open_inner(path, 0, Some(if q1 { "q1" } else { "q0" }))
    }
    fn open_inner(path: &Path, min_mapq: u8, partition: Option<&str>) -> Result<Self> {
        let metadata = Metadata::open(path)?;
        let kind = metadata.supported_kind()?;
        let shard_scoped = kind == Kind::Concat && metadata.shard_scoped();
        let mut contigs = vec![];
        for line in fs::read_to_string(path.join("_contigsizes"))?.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(fields.len() == 2, "invalid contig sizes line");
            ensure!(
                !contigs.iter().any(|c: &Contig| c.name == fields[0]),
                "duplicate contig"
            );
            contigs.push(Contig {
                name: fields[0].into(),
                length: fields[1].parse()?,
            });
        }
        let mut files = fs::read_dir(path.join(partition.unwrap_or(if min_mapq == 0 || shard_scoped {
            "q0"
        } else {
            "q1"
        })))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
        files.retain(|p| p.extension().is_some_and(|s| s == "parquet"));
        files.sort_by(|a, b| {
            let id = |p: &PathBuf| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
            };
            match (id(a), id(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => a.cmp(b),
            }
        });
        let contig_ids = contigs
            .iter()
            .enumerate()
            .map(|(i, c)| Ok((c.name.clone(), u32::try_from(i)?)))
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self {
            kind,
            contigs,
            files: files.into_iter(),
            min_mapq,
            contig_ids,
            shard_scoped,
            next_read_id: 1,
        })
    }
    fn next_frame(&mut self) -> Result<Option<DataFrame>> {
        let Some(path) = self.files.next() else {
            return Ok(None);
        };
        let mut df = ParquetReader::new(File::open(path)?).finish()?;
        if self.shard_scoped {
            let raw = df
                .column("read_idx")?
                .as_materialized_series()
                .cast(&DataType::UInt64)?;
            let mut previous = None;
            let mut mapped = Vec::with_capacity(df.height());
            let mut id = 0;
            for raw_id in raw.u64()?.into_iter() {
                let raw_id = raw_id.context("null concat read ID")?;
                if previous != Some(raw_id) {
                    id = self.next_read_id;
                    self.next_read_id = id.checked_add(1).context("logical read ID overflow")?;
                    previous = Some(raw_id);
                }
                mapped.push(id);
            }
            df.replace("read_idx", Series::new("read_idx".into(), mapped))?;
        }
        let quality = if self.kind == Kind::Pairs {
            "mapq"
        } else {
            "mapping_quality"
        };
        if self.min_mapq > 0 {
            df = df.filter(&df.column(quality)?.u8()?.gt_eq(self.min_mapq))?;
        }
        Ok(Some(df))
    }
    pub fn next_batch(&mut self) -> Result<Option<Batch>> {
        let Some(df) = self.next_frame()? else {
            return Ok(None);
        };
        self.frame_batch(df).map(Some)
    }
    fn frame_batch(&self, df: DataFrame) -> Result<Batch> {
        let quality = if self.kind == Kind::Pairs {
            "mapq"
        } else {
            "mapping_quality"
        };
        // Resolve columns once per shard. Native integer buffers are shared,
        // and only output-owned strings (read IDs/filter reasons) are allocated.
        let number = |name: &str| Numbers::new(df.column(name)?);
        let chromosome = |name: &str| {
            Codes::new(df.column(name)?, |s| {
                self.contig_ids
                    .get(s)
                    .copied()
                    .context("unknown contig in shard")
            })
        };
        let strand = |name: &str| {
            Codes::new(df.column(name)?, |s| {
                ensure!(s == "+" || s == "-", "invalid strand in PQS");
                Ok(s.as_bytes()[0])
            })
        };
        let mapq = number(quality)?;
        if self.kind == Kind::Pairs {
            let read_ids = df.column("read_idx")?.cast(&DataType::String)?;
            let read_ids = read_ids.str()?;
            let chrom1 = chromosome("chrom1")?;
            let chrom2 = chromosome("chrom2")?;
            let pos1 = number("pos1")?;
            let pos2 = number("pos2")?;
            let strand1 = strand("strand1")?;
            let strand2 = strand("strand2")?;
            let rows = (0..df.height())
                .map(|i| {
                    Ok(Pair {
                        read_id: read_ids.get(i).context("null read ID")?.to_owned(),
                        chrom1: chrom1.at(i)?,
                        chrom2: chrom2.at(i)?,
                        pos1: pos1.at(i)?,
                        pos2: pos2.at(i)?,
                        strand1: strand1.at(i)?,
                        strand2: strand2.at(i)?,
                        mapq: mapq.at(i)?.try_into()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Batch::Pairs(rows))
        } else {
            let read_idx = number("read_idx")?;
            let read_length = number("read_length")?;
            let read_start = number("read_start")?;
            let read_end = number("read_end")?;
            let strand = strand("strand")?;
            let chrom = chromosome("chrom")?;
            let start = number("start")?;
            let end = number("end")?;
            let identity = df.column("identity")?.cast(&DataType::Float32)?;
            let identity = identity.f32()?;
            let reasons = df.column("filter_reason")?.cast(&DataType::String)?;
            let reasons = reasons.str()?;
            let rows = (0..df.height())
                .map(|i| {
                    Ok(Alignment {
                        read_idx: read_idx.at(i)?,
                        read_length: read_length.at(i)?.try_into()?,
                        read_start: read_start.at(i)?.try_into()?,
                        read_end: read_end.at(i)?.try_into()?,
                        strand: strand.at(i)?,
                        chrom: chrom.at(i)?,
                        start: start.at(i)?,
                        end: end.at(i)?,
                        mapping_quality: mapq.at(i)?.try_into()?,
                        identity: identity.get(i).context("null identity")?,
                        filter_reason: reasons.get(i).context("null filter reason")?.to_owned(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Batch::Concat(rows))
        }
    }
}
// ChunkedArray clones share their buffers; normal PQS integer columns never
// need to be expanded to u64 solely to construct row records.
enum Numbers {
    U8(UInt8Chunked),
    U32(UInt32Chunked),
    U64(UInt64Chunked),
}
impl Numbers {
    fn new(column: &Column) -> Result<Self> {
        Ok(match column.dtype() {
            DataType::UInt8 => Self::U8(column.u8()?.clone()),
            DataType::UInt32 => Self::U32(column.u32()?.clone()),
            DataType::UInt64 => Self::U64(column.u64()?.clone()),
            _ => Self::U64(column.cast(&DataType::UInt64)?.u64()?.clone()),
        })
    }
    fn at(&self, i: usize) -> Result<u64> {
        match self {
            Self::U8(c) => c.get(i).map(u64::from),
            Self::U32(c) => c.get(i).map(u64::from),
            Self::U64(c) => c.get(i),
        }
        .context("null integer in PQS")
    }
}
enum Codes<T> {
    Local {
        indices: UInt32Chunked,
        values: Vec<Option<T>>,
    },
    Plain(Vec<T>),
}
impl<T: Copy> Codes<T> {
    fn new(column: &Column, decode: impl Fn(&str) -> Result<T>) -> Result<Self> {
        if matches!(column.dtype(), DataType::Categorical(_, _)) {
            let cat = column.as_materialized_series().categorical()?;
            let rev = cat.get_rev_map();
            if rev.is_local() {
                // Unused dictionary entries may contain values removed by a
                // quality filter. Validate only codes that are actually read.
                let values = rev
                    .get_categories()
                    .values_iter()
                    .map(|s| decode(s).ok())
                    .collect();
                return Ok(Self::Local {
                    indices: cat.physical().clone(),
                    values,
                });
            }
        }
        let strings = column.cast(&DataType::String)?;
        let values = strings
            .str()?
            .into_iter()
            .map(|s| decode(s.context("null categorical value")?))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::Plain(values))
    }
    fn at(&self, i: usize) -> Result<T> {
        match self {
            Self::Local { indices, values } => {
                let code = indices.get(i).context("null categorical code")?;
                values
                    .get(code as usize)
                    .copied()
                    .flatten()
                    .context("invalid contig or strand in categorical column")
            }
            Self::Plain(values) => values.get(i).copied().context("missing categorical value"),
        }
    }
}
fn metadata(kind: Kind, chunksize: usize, contigs: &[Contig]) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    match kind {
        Kind::Pairs => {
            let pos = if pos_dtype(contigs) == DataType::UInt32 {
                "UInt32"
            } else {
                "UInt64"
            };
            include_str!("pairs.metadata")
                .replace("REPLACE", &now)
                .replace("CHUNKSIZE", &chunksize.to_string())
                .replace("pos_type_lower", &pos.to_lowercase())
                .replace("pos_type", pos)
        }
        Kind::Concat => include_str!("concat.metadata")
            .replace("{creation_time}", &now)
            .replace("{chunksize}", &chunksize.to_string())
            .replace("{position_dtype}", "uint64")
            .replace("{position_type}", "UInt64"),
    }
}
