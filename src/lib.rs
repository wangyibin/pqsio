//! PQS storage. Coordinates are passed through unchanged; pairs uses 1-based
//! positions, concat uses 0-based half-open reference and read intervals.
use anyhow::{bail, ensure, Context, Result};
use polars::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    path::{Path, PathBuf},
};
pub mod ffi;

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

/// Synchronous bounded writer. Concat calls each submit exactly one complete
/// read, with strictly increasing global read IDs. A single oversized read is
/// allowed to exceed the shard target, but is never split.
pub struct Writer {
    kind: Kind,
    contigs: Vec<Contig>,
    target: PathBuf,
    staging: PathBuf,
    chunk_size: usize,
    pairs: Vec<Pair>,
    concat: Vec<Alignment>,
    last_read: Option<u64>,
    shard: usize,
    counts: Counts,
    finished: bool,
    failed: bool,
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
            pairs: vec![],
            concat: vec![],
            last_read: None,
            shard: 0,
            counts: Counts::default(),
            finished: false,
            failed: false,
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
    pub fn write_pairs(&mut self, rows: &[Pair]) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Pairs, "requires pairs writer");
        for r in rows {
            self.position(r.chrom1, r.pos1)?;
            self.position(r.chrom2, r.pos2)?;
            ensure!(r.pos1 > 0 && r.pos2 > 0, "pairs positions must be 1-based");
            ensure!(
                strand_ok(r.strand1) && strand_ok(r.strand2),
                "strand must be + or -"
            );
        }
        for r in rows {
            self.pairs.push(r.clone());
            if self.pairs.len() >= self.chunk_size {
                self.flush()?;
            }
        }
        Ok(())
    }
    pub fn write_read(&mut self, rows: &[Alignment]) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Concat, "requires concat writer");
        let first = rows
            .first()
            .context("a complete read must contain at least one alignment")?;
        ensure!(
            self.last_read.is_none_or(|id| first.read_idx > id),
            "read IDs must be strictly increasing; submit each complete read once"
        );
        for r in rows {
            ensure!(
                r.read_idx == first.read_idx && r.read_length == first.read_length,
                "read ID and length must agree within a complete read"
            );
            ensure!(
                r.read_start < r.read_end && r.read_end <= r.read_length,
                "invalid read interval"
            );
            ensure!(r.start < r.end, "invalid reference interval");
            self.position(r.chrom, r.end)?;
            ensure!(
                strand_ok(r.strand) && r.identity.is_finite(),
                "invalid strand or non-finite identity"
            );
        }
        if !self.concat.is_empty() && self.concat.len().saturating_add(rows.len()) > self.chunk_size
        {
            self.flush()?;
        }
        self.concat.extend_from_slice(rows);
        self.last_read = Some(first.read_idx);
        if self.concat.len() >= self.chunk_size {
            self.flush()?;
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
        let mut frame = match self.kind {
            Kind::Pairs => pair_frame(&self.pairs, &self.contigs)?,
            Kind::Concat => concat_frame(&self.concat, &self.contigs)?,
        };
        let mq = if self.kind == Kind::Pairs {
            "mapq"
        } else {
            "mapping_quality"
        };
        let mut q1 = frame.filter(&frame.column(mq)?.u8()?.gt_eq(1))?;
        ParquetWriter::new(File::create(
            self.staging.join(format!("q0/{}.parquet", self.shard)),
        )?)
        .finish(&mut frame)?;
        if q1.height() > 0 {
            ParquetWriter::new(File::create(
                self.staging.join(format!("q1/{}.parquet", self.shard)),
            )?)
            .finish(&mut q1)?;
        }
        self.counts.q0_records += frame.height() as u64;
        self.counts.q1_records += q1.height() as u64;
        if self.kind == Kind::Concat {
            self.counts.q0_concats += self
                .concat
                .iter()
                .map(|r| r.read_idx)
                .collect::<HashSet<_>>()
                .len() as u64;
            self.counts.q1_concats += self
                .concat
                .iter()
                .filter(|r| r.mapping_quality > 0)
                .map(|r| r.read_idx)
                .collect::<HashSet<_>>()
                .len() as u64;
        }
        self.pairs.clear();
        self.concat.clear();
        self.shard += 1;
        Ok(())
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
fn pair_frame(rows: &[Pair], contigs: &[Contig]) -> Result<DataFrame> {
    let mut df = DataFrame::new(vec![
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
    ])?;
    for name in ["pos1", "pos2"] {
        let s = df
            .column(name)?
            .as_materialized_series()
            .cast(&pos_dtype(contigs))?;
        df.replace(name, s)?;
    }
    categorize(&mut df, &["chrom1", "chrom2", "strand1", "strand2"])?;
    Ok(df)
}
fn concat_frame(rows: &[Alignment], contigs: &[Contig]) -> Result<DataFrame> {
    let mut df = DataFrame::new(vec![
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
    ])?;
    categorize(&mut df, &["strand", "chrom", "filter_reason"])?;
    Ok(df)
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
        let path = path.as_ref();
        let meta = fs::read_to_string(path.join("_metadata"))?;
        let compact: String = meta
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| if c == '"' { '\'' } else { c })
            .collect();
        let kind = if compact.contains("'format':'concat'") {
            Kind::Concat
        } else if compact.contains("'format':'pairs'") {
            Kind::Pairs
        } else {
            bail!("unsupported PQS format");
        };
        ensure!(compact.contains("'is_pqs':True"), "not a PQS dataset");
        let version = if kind == Kind::Pairs {
            "'format-version':'0.1.0'"
        } else {
            "'format-version':'0.2.0'"
        };
        ensure!(compact.contains(version), "unsupported PQS format version");
        let shard_scoped = kind == Kind::Concat && compact.contains("'read_idx_scope':'shard'");
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
        let mut files = fs::read_dir(path.join(if min_mapq == 0 || shard_scoped {
            "q0"
        } else {
            "q1"
        }))?
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
    pub fn next_batch(&mut self) -> Result<Option<Batch>> {
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
        let string_names: &[&str] = if self.kind == Kind::Pairs {
            &["read_idx", "chrom1", "chrom2", "strand1", "strand2"]
        } else {
            &["strand", "chrom", "filter_reason"]
        };
        for n in string_names {
            let s = df
                .column(n)?
                .as_materialized_series()
                .cast(&DataType::String)?;
            df.replace(n, s)?;
        }
        let numeric_names: &[&str] = if self.kind == Kind::Pairs {
            &["pos1", "pos2", "mapq"]
        } else {
            &[
                "read_idx",
                "read_length",
                "read_start",
                "read_end",
                "start",
                "end",
                "mapping_quality",
            ]
        };
        for n in numeric_names {
            let s = df
                .column(n)?
                .as_materialized_series()
                .cast(&DataType::UInt64)?;
            df.replace(n, s)?;
        }
        let text = |n: &str, i| -> Result<String> {
            Ok(df
                .column(n)?
                .str()?
                .get(i)
                .context("null string in PQS")?
                .into())
        };
        let num = |n: &str, i| -> Result<u64> {
            df.column(n)?.u64()?.get(i).context("null integer in PQS")
        };
        let chrom = |n: &str, i| -> Result<u32> {
            let s = text(n, i)?;
            Ok(*self.contig_ids.get(&s).context("unknown contig in shard")?)
        };
        let strand = |n: &str, i| -> Result<u8> {
            let s = text(n, i)?;
            ensure!(s == "+" || s == "-", "invalid strand in PQS");
            Ok(s.as_bytes()[0])
        };
        if self.kind == Kind::Pairs {
            let rows = (0..df.height())
                .map(|i| {
                    Ok(Pair {
                        read_id: text("read_idx", i)?,
                        chrom1: chrom("chrom1", i)?,
                        pos1: num("pos1", i)?,
                        chrom2: chrom("chrom2", i)?,
                        pos2: num("pos2", i)?,
                        strand1: strand("strand1", i)?,
                        strand2: strand("strand2", i)?,
                        mapq: num("mapq", i)?.try_into()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Some(Batch::Pairs(rows)))
        } else {
            let identity = df
                .column("identity")?
                .as_materialized_series()
                .cast(&DataType::Float32)?;
            let rows = (0..df.height())
                .map(|i| {
                    Ok(Alignment {
                        read_idx: num("read_idx", i)?,
                        read_length: num("read_length", i)?.try_into()?,
                        read_start: num("read_start", i)?.try_into()?,
                        read_end: num("read_end", i)?.try_into()?,
                        strand: strand("strand", i)?,
                        chrom: chrom("chrom", i)?,
                        start: num("start", i)?,
                        end: num("end", i)?,
                        mapping_quality: num("mapping_quality", i)?.try_into()?,
                        identity: identity.f32()?.get(i).context("null identity")?,
                        filter_reason: text("filter_reason", i)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Some(Batch::Concat(rows)))
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
