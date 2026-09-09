//! Optional per-quality row-group summaries. No record/read locator and no summary cache.
use crate::metadata::{obj, Value};
use crate::*;
use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

const ROOT: &str = ".pqsio-index";
const LIMIT: u64 = 16 * 1024 * 1024;
const FORMAT: &str = "pqsio-region-1";
const ALGORITHM: &str = "contig-endpoint-envelope-1";

/// Data partition for independent row-group index construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexQuality {
    Q0,
    Q1,
}
impl IndexQuality {
    fn name(self) -> &'static str {
        match self {
            Self::Q0 => "q0",
            Self::Q1 => "q1",
        }
    }
    fn root(self, path: &Path) -> PathBuf {
        let root = path.join(ROOT);
        match self {
            Self::Q0 => root,
            Self::Q1 => root.join("q1"),
        }
    }
    fn source(self, path: &Path) -> Result<Reader> {
        Reader::open_partition(path, self == Self::Q1)
    }
}

#[derive(Clone, Debug)]
pub struct Region {
    pub contig: String,
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexMode {
    Auto,
    Off,
    Require,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairsMode {
    Either,
    Both,
}
#[derive(Clone, Debug)]
pub struct QueryOptions {
    pub regions: Vec<Region>,
    pub min_mapq: u8,
    pub index: IndexMode,
    pub pairs_mode: PairsMode,
    pub read: ReadOptions,
}
impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            regions: vec![],
            min_mapq: 0,
            index: IndexMode::Auto,
            pairs_mode: PairsMode::Either,
            read: ReadOptions::default(),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct QueryStats {
    /// Selected data partition; all row-group/row counters refer to it.
    pub source_quality: &'static str,
    pub index_used: bool,
    pub fallback_reason: Option<String>,
    pub total_row_groups: u64,
    /// Candidates encountered so far, not a precomputed unbounded list.
    pub candidate_row_groups: u64,
    pub decoded_row_groups: u64,
    pub skipped_row_groups: u64,
    pub decoded_rows: u64,
    pub returned_rows: u64,
    pub complete: bool,
    pub manifest_bytes: u64,
    pub source_inventory_bytes: u64,
    pub summary_cache_bytes: u64,
}
impl QueryStats {
    pub fn to_json(&self) -> String {
        obj([
            ("source_quality", Value::from(self.source_quality)),
            ("index_used", Value::Bool(self.index_used)),
            (
                "fallback_reason",
                self.fallback_reason
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("total_row_groups", self.total_row_groups.into()),
            ("candidate_row_groups", self.candidate_row_groups.into()),
            ("decoded_row_groups", self.decoded_row_groups.into()),
            ("skipped_row_groups", self.skipped_row_groups.into()),
            ("decoded_rows", self.decoded_rows.into()),
            ("returned_rows", self.returned_rows.into()),
            ("complete", Value::Bool(self.complete)),
            ("manifest_bytes", self.manifest_bytes.into()),
            ("source_inventory_bytes", self.source_inventory_bytes.into()),
            ("summary_cache_bytes", self.summary_cache_bytes.into()),
        ])
        .json()
    }
}
#[derive(Clone)]
pub(crate) struct Predicate {
    regions: BTreeMap<u32, Vec<(u64, u64)>>,
    both: bool,
    min_mapq: u8,
}
impl Predicate {
    fn new(contigs: &[Contig], options: &QueryOptions) -> Result<Self> {
        let mut regions: BTreeMap<u32, Vec<(u64, u64)>> = BTreeMap::new();
        for r in &options.regions {
            let id = contigs
                .iter()
                .position(|c| c.name == r.contig)
                .context("query contig does not exist")?;
            ensure!(
                r.start < r.end && r.end <= contigs[id].length,
                "query requires 0 <= start < end <= contig length: {}",
                r.contig
            );
            regions.entry(id as u32).or_default().push((r.start, r.end));
        }
        for intervals in regions.values_mut() {
            intervals.sort_unstable();
            let mut merged: Vec<(u64, u64)> = vec![];
            for &(a, b) in intervals.iter() {
                if let Some(last) = merged.last_mut().filter(|last| a <= last.1) {
                    last.1 = last.1.max(b);
                } else {
                    merged.push((a, b));
                }
            }
            *intervals = merged;
        }
        Ok(Self {
            regions,
            both: options.pairs_mode == PairsMode::Both,
            min_mapq: options.min_mapq,
        })
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
    fn overlap(&self, chrom: u32, start: u64, end: u64) -> bool {
        self.regions.get(&chrom).is_some_and(|rs| {
            let i = rs.partition_point(|&(_, b)| b <= start);
            rs.get(i).is_some_and(|&(a, _)| a < end)
        })
    }
    pub(crate) fn pair(&self, c: &PairColumns, i: usize) -> bool {
        let hit = |chrom, pos: u64| {
            pos.checked_sub(1)
                .is_some_and(|a| self.overlap(chrom, a, pos))
        };
        let a = hit(c.chrom1[i], c.pos1[i]);
        let b = hit(c.chrom2[i], c.pos2[i]);
        c.mapq[i] >= self.min_mapq && if self.both { a && b } else { a || b }
    }
    pub(crate) fn alignment(&self, c: &ConcatColumns, i: usize) -> bool {
        c.mapping_quality[i] >= self.min_mapq && self.overlap(c.chrom[i], c.start[i], c.end[i])
    }
}

// FNV-1a is a corruption/identity checksum, NOT a cryptographic guarantee.
fn hash(mut input: impl Read) -> Result<u64> {
    let mut h = 0xcbf29ce484222325u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    Ok(h)
}
fn footer_hash(path: &Path) -> Result<u64> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    ensure!(size >= 12, "invalid Parquet file: {}", path.display());
    f.seek(SeekFrom::End(-8))?;
    let mut tail = [0; 8];
    f.read_exact(&mut tail)?;
    ensure!(&tail[4..] == b"PAR1", "invalid Parquet footer");
    let len = u32::from_le_bytes(tail[..4].try_into().unwrap()) as u64 + 8;
    ensure!(len <= size - 4, "invalid Parquet footer length");
    f.seek(SeekFrom::Start(size - len))?;
    hash(f.take(len))
}
struct Snapshot {
    value: Value,
    files: Vec<PathBuf>,
    groups: Vec<u64>,
    total: u64,
}
fn snapshot(path: &Path, source: &Reader) -> Result<Snapshot> {
    let files = source.files.as_slice().to_vec();
    let mut entries = vec![];
    let mut groups = vec![];
    let mut total = 0;
    for file in &files {
        let m = fs::metadata(file)?;
        let modified = m.modified()?.duration_since(std::time::UNIX_EPOCH)?;
        let mut r = ParquetReader::new(File::open(file)?);
        let n = r.get_metadata()?.row_groups.len() as u64;
        groups.push(n);
        total += n;
        entries.push(obj([
            (
                "name",
                Value::from(
                    file.file_name()
                        .and_then(|s| s.to_str())
                        .context("non-UTF8 shard name")?,
                ),
            ),
            ("size", m.len().into()),
            ("mtime_secs", modified.as_secs().into()),
            ("mtime_nanos", (modified.subsec_nanos() as u64).into()),
            ("footer_fnv1a64", footer_hash(file)?.into()),
            ("row_groups", n.into()),
        ]));
    }
    let meta = Metadata::open(path)?;
    let counts = match File::open(path.join("_metadata_counts")) {
        Ok(file) => hash(file)?.into(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::from("absent"),
        Err(e) => return Err(e.into()),
    };
    let value = obj([
        ("counts_fnv1a64", counts),
        (
            "format",
            Value::from(meta.get_str("format").context("missing format")?),
        ),
        (
            "format_version",
            Value::from(
                meta.get_str("format-version")
                    .context("missing format version")?,
            ),
        ),
        (
            "metadata_fnv1a64",
            hash(File::open(path.join("_metadata"))?)?.into(),
        ),
        (
            "contigs_fnv1a64",
            hash(File::open(path.join("_contigsizes"))?)?.into(),
        ),
        ("shards", Value::List(entries)),
    ]);
    Ok(Snapshot {
        value,
        files,
        groups,
        total,
    })
}
fn read_manifest(path: &Path) -> Result<Metadata> {
    let f = File::open(path)?;
    ensure!(
        f.metadata()?.len() <= LIMIT,
        "index manifest exceeds 16 MiB limit"
    );
    let mut s = String::new();
    f.take(LIMIT + 1).read_to_string(&mut s)?;
    Metadata::parse(&s)
}
fn u64_field(m: &BTreeMap<String, Value>, name: &str) -> Result<u64> {
    m.get(name)
        .and_then(Value::u64)
        .with_context(|| format!("invalid index {name}"))
}
fn write_num(w: &mut impl Write, n: u64) -> Result<()> {
    w.write_all(&n.to_le_bytes())?;
    Ok(())
}
fn read_num(r: &mut impl Read) -> Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// A generation is published by replacing CURRENT only after all checks pass.
/// Old generations remain valid for readers already using them; see docs/query.md.
pub fn build_index(path: impl AsRef<Path>, rebuild: bool) -> Result<()> {
    build_index_for_quality(path, IndexQuality::Q0, rebuild)
}
/// Build one independent q0/q1 index. The legacy build_index remains q0-only.
pub fn build_index_for_quality(
    path: impl AsRef<Path>,
    quality: IndexQuality,
    rebuild: bool,
) -> Result<()> {
    build_impl(path.as_ref(), quality, rebuild, &mut || Ok(()))
}
fn build_impl(
    path: &Path,
    quality: IndexQuality,
    rebuild: bool,
    before_publish: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let source = quality.source(path)?;
    let before = snapshot(path, &source)?;
    let root = quality.root(path);
    fs::create_dir_all(&root)?;
    let lock = root.join("BUILD.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .context("index build locked; another build may be active")?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _lock = Cleanup(lock);
    let _lock_file = lock_file;
    ensure!(
        rebuild || !root.join("CURRENT").exists(),
        "index already exists; pass rebuild=true"
    );
    let tag = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let temp = root.join(format!("{tag}.partial"));
    fs::create_dir(&temp)?;
    let published = root.join(&tag);
    let pointer = root.join(format!("{tag}.current"));
    let result = (|| {
        let mut parts = vec![];
        for (s, file) in before.files.iter().enumerate() {
            let mut reader = ParquetReader::new(File::open(file)?);
            let metadata = reader.get_metadata()?.clone();
            let part = temp.join(format!("{s}.rg"));
            let mut output = BufWriter::with_capacity(65536, File::create(&part)?);
            let mut offset = 0u64;
            for (g, group) in metadata.row_groups.iter().enumerate() {
                let rows = group.num_rows() as u64;
                let mut summary: BTreeMap<(u32, u8), (u64, u64, u8)> = BTreeMap::new();
                if rows > 0 {
                    let mut one = metadata.as_ref().clone();
                    one.num_rows = group.num_rows();
                    one.row_groups = vec![group.clone()];
                    let mut decoder =
                        ParquetReader::new(File::open(file)?).read_parallel(ParallelStrategy::None);
                    decoder.set_metadata(Arc::new(one));
                    let columns = source.frame_columns(decoder.finish()?)?;
                    let mut add =
                        |chrom: u32, endpoint: u8, start: u64, end: u64, q: u8| -> Result<()> {
                            let contig = source
                                .contigs
                                .get(chrom as usize)
                                .context("index contig out of range")?;
                            ensure!(
                                start < end && end <= contig.length,
                                "untrustworthy index coordinates in shard {s}, row group {g}"
                            );
                            let e = summary.entry((chrom, endpoint)).or_insert((start, end, q));
                            e.0 = e.0.min(start);
                            e.1 = e.1.max(end);
                            e.2 = e.2.max(q);
                            Ok(())
                        };
                    match &columns {
                        ColumnBatch::Pairs(c) => {
                            for i in 0..c.pos1.len() {
                                add(
                                    c.chrom1[i],
                                    1,
                                    c.pos1[i]
                                        .checked_sub(1)
                                        .context("pairs position must be >= 1")?,
                                    c.pos1[i],
                                    c.mapq[i],
                                )?;
                                add(
                                    c.chrom2[i],
                                    2,
                                    c.pos2[i]
                                        .checked_sub(1)
                                        .context("pairs position must be >= 1")?,
                                    c.pos2[i],
                                    c.mapq[i],
                                )?;
                            }
                        }
                        ColumnBatch::Concat(c) => {
                            for i in 0..c.start.len() {
                                add(c.chrom[i], 0, c.start[i], c.end[i], c.mapping_quality[i])?;
                            }
                        }
                    }
                }
                // Header: group ordinal, original rows, shard row offset, summary count.
                for n in [g as u64, rows, offset, summary.len() as u64] {
                    write_num(&mut output, n)?;
                }
                for ((contig, endpoint), (start, end, q)) in summary {
                    // 32 bytes: u32 contig, u8 endpoint, u8 MAPQ, 2 reserved; u64 start/end; 8 reserved.
                    output.write_all(&contig.to_le_bytes())?;
                    output.write_all(&[endpoint, q, 0, 0])?;
                    write_num(&mut output, start)?;
                    write_num(&mut output, end)?;
                    write_num(&mut output, 0)?;
                }
                offset = offset.checked_add(rows).context("row offset overflow")?;
            }
            output.flush()?;
            output.get_ref().sync_all()?;
            drop(output);
            parts.push(obj([
                ("bytes", fs::metadata(&part)?.len().into()),
                ("fnv1a64", hash(File::open(&part)?)?.into()),
            ]));
        }
        before_publish()?;
        let after_source = quality.source(path)?;
        ensure!(
            snapshot(path, &after_source)?.value == before.value,
            "source changed during index build; index not published"
        );
        let manifest = obj([
            ("version", Value::from(FORMAT)),
            ("algorithm", Value::from(ALGORITHM)),
            (
                "coordinates",
                Value::from("0-based-half-open; pairs=[pos-1,pos)"),
            ),
            ("source", before.value.clone()),
            ("complete", 1u64.into()),
            (
                "build",
                obj([
                    ("quality", Value::from(quality.name())),
                    ("partition", Value::from("shard")),
                    ("summary_cache_bytes", 0u64.into()),
                ]),
            ),
            ("parts", Value::List(parts)),
        ])
        .json();
        ensure!(
            manifest.len() as u64 <= LIMIT,
            "index manifest exceeds 16 MiB limit"
        );
        let mut f = File::create(temp.join("manifest.json"))?;
        f.write_all(manifest.as_bytes())?;
        f.sync_all()?;
        fs::rename(&temp, &published)?;
        let mut f = File::create(&pointer)?;
        f.write_all(tag.as_bytes())?;
        f.sync_all()?;
        fs::rename(&pointer, root.join("CURRENT"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
        let _ = fs::remove_dir_all(&published);
        let _ = fs::remove_file(&pointer);
    }
    result
}

pub(crate) struct IndexCursor {
    directory: PathBuf,
    shard: usize,
    input: Option<BufReader<File>>,
}
impl IndexCursor {
    fn open(path: &Path, snapshot: &Snapshot, quality: IndexQuality) -> Result<(Self, u64)> {
        let root = quality.root(path);
        let mut tag = String::new();
        File::open(root.join("CURRENT"))?
            .take(129)
            .read_to_string(&mut tag)?;
        ensure!(
            !tag.is_empty()
                && tag.len() <= 128
                && tag.bytes().all(|c| c.is_ascii_digit() || c == b'-'),
            "invalid index CURRENT"
        );
        let directory = root.join(tag);
        let manifest_path = directory.join("manifest.json");
        let m = read_manifest(&manifest_path)?;
        ensure!(
            m.get_str("version") == Some(FORMAT) && m.get_str("algorithm") == Some(ALGORITHM),
            "unsupported index version or algorithm"
        );
        ensure!(
            m.get_str("coordinates") == Some("0-based-half-open; pairs=[pos-1,pos)")
                && m.fields.get("complete").and_then(Value::u64) == Some(1),
            "incomplete or invalid index"
        );
        ensure!(
            m.fields
                .get("build")
                .and_then(Value::object)
                .and_then(|b| b.get("quality"))
                .and_then(Value::string)
                == Some(quality.name()),
            "index quality mismatch: expected {}",
            quality.name()
        );
        ensure!(
            m.fields.get("source") == Some(&snapshot.value),
            "stale index: source identity changed"
        );
        let Some(Value::List(parts)) = m.fields.get("parts") else {
            bail!("invalid index parts");
        };
        ensure!(
            parts.len() == snapshot.files.len(),
            "invalid index partition count"
        );
        // Preflight ALL partitions before yielding anything. A late index error is
        // terminal, never a restart that could duplicate previously emitted rows.
        for (s, part) in parts.iter().enumerate() {
            let p = part.object().context("invalid index partition")?;
            let file = directory.join(format!("{s}.rg"));
            ensure!(
                fs::metadata(&file)?.len() == u64_field(p, "bytes")?
                    && hash(File::open(&file)?)? == u64_field(p, "fnv1a64")?,
                "corrupt index partition {s}"
            );
            let mut r = BufReader::with_capacity(65536, File::open(&file)?);
            let mut offset = 0;
            for g in 0..snapshot.groups[s] {
                ensure!(read_num(&mut r)? == g, "invalid index group ordinal");
                let rows = read_num(&mut r)?;
                ensure!(read_num(&mut r)? == offset, "invalid index row offset");
                offset = offset
                    .checked_add(rows)
                    .context("index row offset overflow")?;
                let n = read_num(&mut r)?;
                ensure!(
                    n <= (fs::metadata(&file)?.len() / 32),
                    "invalid summary count"
                );
                ensure!((rows == 0) == (n == 0), "invalid empty group summary");
                for _ in 0..n {
                    let mut b = [0; 32];
                    r.read_exact(&mut b)?;
                }
            }
            let mut extra = [0];
            ensure!(r.read(&mut extra)? == 0, "index trailing data");
        }
        Ok((
            Self {
                directory,
                shard: usize::MAX,
                input: None,
            },
            fs::metadata(manifest_path)?.len(),
        ))
    }
    pub(crate) fn candidate(
        &mut self,
        shard: usize,
        group: usize,
        rows: usize,
        predicate: &Predicate,
    ) -> Result<bool> {
        if self.shard != shard {
            self.input = Some(BufReader::with_capacity(
                65536,
                File::open(self.directory.join(format!("{shard}.rg")))?,
            ));
            self.shard = shard;
        }
        let r = self.input.as_mut().unwrap();
        ensure!(
            read_num(r)? == group as u64 && read_num(r)? == rows as u64,
            "index/source group mismatch during query"
        );
        let _offset = read_num(r)?;
        let n = read_num(r)?;
        let mut a = false;
        let mut b = false;
        for _ in 0..n {
            let mut head = [0; 8];
            r.read_exact(&mut head)?;
            let chrom = u32::from_le_bytes(head[..4].try_into().unwrap());
            let start = read_num(r)?;
            let end = read_num(r)?;
            let _reserved = read_num(r)?;
            if head[5] >= predicate.min_mapq && predicate.overlap(chrom, start, end) {
                match head[4] {
                    0 | 1 => a = true,
                    2 => b = true,
                    _ => bail!("invalid index endpoint"),
                }
            }
        }
        Ok(if predicate.both { a && b } else { a || b })
    }
}
pub(crate) struct QueryState {
    pub predicate: Predicate,
    pub cursor: Option<IndexCursor>,
    pub stats: QueryStats,
}
/// QueryReader uses the same stream, exact predicate, ID mapping and batching
/// in indexed and sequential modes. Existing stream ABI remains unchanged.
pub struct QueryReader {
    inner: StreamingReader,
}
impl QueryReader {
    pub fn open(path: impl AsRef<Path>, options: QueryOptions) -> Result<Self> {
        Self::open_stream(path, options).map(|inner| Self { inner })
    }
    pub(crate) fn open_stream(
        path: impl AsRef<Path>,
        options: QueryOptions,
    ) -> Result<StreamingReader> {
        let path = path.as_ref();
        let metadata = Metadata::open(path)?;
        let kind = metadata.supported_kind()?;
        let use_q1 = options.min_mapq > 0
            && options.read.concat_filter != Some(ConcatFilter::CompleteReads)
            && !(kind == Kind::Concat && metadata.shard_scoped());
        let quality = if use_q1 {
            IndexQuality::Q1
        } else {
            IndexQuality::Q0
        };
        let source = quality.source(path)?;
        let mut inner = StreamingReader::from_source(source, options.min_mapq, options.read)?;
        ensure!(
            inner.kind() == Kind::Pairs || options.pairs_mode == PairsMode::Either,
            "pairs_mode=both requires pairs"
        );
        let predicate = Predicate::new(inner.contigs(), &options)?;
        // Source errors are outside the fallback handler.
        let snap = snapshot(path, &inner.source)?;
        let mut stats = QueryStats {
            source_quality: quality.name(),
            total_row_groups: snap.total,
            source_inventory_bytes: snap.value.json().len() as u64,
            ..Default::default()
        };
        let bypass = if options.index == IndexMode::Off {
            Some("index disabled")
        } else if options.read.concat_filter == Some(ConcatFilter::CompleteReads) {
            Some("complete_reads requires sequential q0 scan")
        } else if inner.source.shard_scoped {
            Some("shard-local concat IDs require sequential mapping")
        } else {
            None
        };
        let mut cursor = None;
        if let Some(reason) = bypass {
            // require checks index availability, but never overrides semantic fallbacks.
            if options.index == IndexMode::Require {
                let (_, bytes) = IndexCursor::open(path, &snap, quality).with_context(|| {
                    format!("required index unavailable for {}", quality.name())
                })?;
                stats.manifest_bytes = bytes;
            }
            stats.fallback_reason = Some(reason.into());
        } else {
            match IndexCursor::open(path, &snap, quality) {
                Ok((index, bytes)) => {
                    cursor = Some(index);
                    stats.index_used = true;
                    stats.manifest_bytes = bytes;
                }
                Err(e) if options.index == IndexMode::Auto => {
                    stats.fallback_reason =
                        Some(format!("{} index unavailable: {e:#}", quality.name()))
                }
                Err(e) => {
                    return Err(
                        e.context(format!("required index unavailable for {}", quality.name()))
                    )
                }
            }
        }
        inner.query = Some(QueryState {
            predicate,
            cursor,
            stats,
        });
        Ok(inner)
    }
    pub fn kind(&self) -> Kind {
        self.inner.kind()
    }
    pub fn contigs(&self) -> &[Contig] {
        self.inner.contigs()
    }
    pub fn stats(&self) -> &QueryStats {
        &self.inner.query.as_ref().unwrap().stats
    }
    pub fn next_columns(&mut self) -> Result<Option<ColumnBatch>> {
        self.inner.next_columns()
    }
    pub fn next_batch(&mut self) -> Result<Option<Batch>> {
        self.inner.next_batch()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/output");
            fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "query-rust-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut writer = Writer::create(
                &path,
                Kind::Pairs,
                vec![Contig {
                    name: "chr1".into(),
                    length: 100,
                }],
                2,
            )
            .unwrap();
            writer
                .write_pairs(&[Pair {
                    read_id: "a".into(),
                    chrom1: 0,
                    pos1: 1,
                    chrom2: 0,
                    pos2: 99,
                    strand1: b'+',
                    strand2: b'-',
                    mapq: 60,
                }])
                .unwrap();
            writer.finish().unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn options(index: IndexMode) -> QueryOptions {
        QueryOptions {
            regions: vec![Region {
                contig: "chr1".into(),
                start: 0,
                end: 1,
            }],
            index,
            ..Default::default()
        }
    }
    #[test]
    fn native_query_matches_sequential() {
        let f = Fixture::new();
        build_index(&f.0, false).unwrap();
        let mut off = QueryReader::open(&f.0, options(IndexMode::Off)).unwrap();
        let mut on = QueryReader::open(&f.0, options(IndexMode::Require)).unwrap();
        assert_eq!(off.next_columns().unwrap(), on.next_columns().unwrap());
        assert!(!on.stats().complete);
        assert!(on.next_columns().unwrap().is_none());
        assert!(on.stats().complete);
    }
    #[test]
    fn failed_build_and_rebuild_preserve_current() {
        let f = Fixture::new();
        assert!(build_impl(&f.0, IndexQuality::Q0, false, &mut || bail!(
            "injected build failure"
        ))
        .is_err());
        assert!(!f.0.join(ROOT).join("CURRENT").exists());
        build_index(&f.0, false).unwrap();
        let pointer = f.0.join(ROOT).join("CURRENT");
        let original = fs::read(&pointer).unwrap();
        assert!(build_impl(&f.0, IndexQuality::Q0, true, &mut || bail!(
            "injected rebuild failure"
        ))
        .is_err());
        assert_eq!(fs::read(&pointer).unwrap(), original);
        assert!(QueryReader::open(&f.0, options(IndexMode::Require)).is_ok());
        assert!(fs::read_dir(f.0.join(ROOT)).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial")));
    }
    #[test]
    fn source_change_before_publish_aborts() {
        let f = Fixture::new();
        build_index(&f.0, false).unwrap();
        let pointer = f.0.join(ROOT).join("CURRENT");
        let original = fs::read(&pointer).unwrap();
        let meta = f.0.join("_metadata");
        let original_meta = fs::read(&meta).unwrap();
        let e = build_impl(&f.0, IndexQuality::Q0, true, &mut || {
            let mut file = fs::OpenOptions::new().append(true).open(&meta)?;
            file.write_all(b"\n")?;
            Ok(())
        })
        .unwrap_err();
        assert!(e.to_string().contains("source changed"));
        assert_eq!(fs::read(&pointer).unwrap(), original);
        fs::write(&meta, original_meta).unwrap();
        assert!(QueryReader::open(&f.0, options(IndexMode::Require)).is_ok());
    }
    #[test]
    fn q1_source_change_aborts_without_replacing_either_index() {
        let f = Fixture::new();
        build_index(&f.0, false).unwrap();
        build_index_for_quality(&f.0, IndexQuality::Q1, false).unwrap();
        let q0 = IndexQuality::Q0.root(&f.0).join("CURRENT");
        let q1 = IndexQuality::Q1.root(&f.0).join("CURRENT");
        let previous = (fs::read(&q0).unwrap(), fs::read(&q1).unwrap());
        let new_shard = f.0.join("q1/1.parquet");
        let error = build_impl(&f.0, IndexQuality::Q1, true, &mut || {
            fs::copy(f.0.join("q1/0.parquet"), &new_shard)?;
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("source changed"));
        assert_eq!((fs::read(&q0).unwrap(), fs::read(&q1).unwrap()), previous);
        fs::remove_file(new_shard).unwrap();
        let mut opts = options(IndexMode::Require);
        opts.min_mapq = 30;
        let mut query = QueryReader::open(&f.0, opts).unwrap();
        assert_eq!(query.stats().source_quality, "q1");
        assert!(query.stats().index_used);
        assert!(query.next_columns().unwrap().is_some());
    }

    #[test]
    fn physical_empty_row_groups_survive_build_and_query() {
        let f = Fixture::new();
        let shard = f.0.join("q0/0.parquet");
        let frame = ParquetReader::new(File::open(&shard).unwrap())
            .finish()
            .unwrap();
        // The normal writer elides empty batches. Its existing low-level
        // row-group API can emit valid zero-page columns without a dependency.
        let mut writer = ParquetWriter::new(File::create(&shard).unwrap())
            .batched(frame.schema())
            .unwrap();
        let empty = (0..frame.width()).map(|_| vec![]).collect::<Vec<_>>();
        writer.write_row_group(&empty).unwrap();
        writer.write_batch(&frame).unwrap();
        writer.write_row_group(&empty).unwrap();
        writer.finish().unwrap();
        drop(writer);
        let mut reader = ParquetReader::new(File::open(&shard).unwrap());
        let metadata = reader.get_metadata().unwrap();
        assert_eq!(
            metadata
                .row_groups
                .iter()
                .map(|g| g.num_rows())
                .collect::<Vec<_>>(),
            vec![0, 1, 0]
        );
        build_index(&f.0, false).unwrap();
        let mut off = QueryReader::open(&f.0, options(IndexMode::Off)).unwrap();
        let mut on = QueryReader::open(&f.0, options(IndexMode::Require)).unwrap();
        assert_eq!(off.next_columns().unwrap(), on.next_columns().unwrap());
        assert!(on.next_columns().unwrap().is_none());
        assert_eq!(on.stats().total_row_groups, 3);
        assert_eq!(on.stats().decoded_row_groups, 1);
        assert_eq!(on.stats().skipped_row_groups, 2);
        fs::copy(&shard, f.0.join("q1/0.parquet")).unwrap();
        build_index_for_quality(&f.0, IndexQuality::Q1, false).unwrap();
        let mut q1_options = options(IndexMode::Require);
        q1_options.min_mapq = 1;
        let mut q1 = QueryReader::open(&f.0, q1_options).unwrap();
        assert!(q1.next_columns().unwrap().is_some());
        assert!(q1.next_columns().unwrap().is_none());
        assert_eq!(q1.stats().source_quality, "q1");
        assert_eq!(q1.stats().total_row_groups, 3);
        assert_eq!(q1.stats().decoded_row_groups, 1);
        assert_eq!(q1.stats().skipped_row_groups, 2);
    }

    #[test]
    fn empty_row_group_wire_record_is_explicit() {
        // Parquet writers may emit zero groups for an empty shard. Exercise an
        // explicit zero-row group header in the index cursor as well.
        let f = Fixture::new();
        let dir = f.0.join("empty-wire");
        fs::create_dir(&dir).unwrap();
        let mut out = File::create(dir.join("0.rg")).unwrap();
        for n in [0, 0, 0, 0] {
            write_num(&mut out, n).unwrap();
        }
        drop(out);
        let mut cursor = IndexCursor {
            directory: dir,
            shard: usize::MAX,
            input: None,
        };
        let source = Reader::open(&f.0, 0).unwrap();
        let predicate = Predicate::new(&source.contigs, &options(IndexMode::Off)).unwrap();
        assert!(!cursor.candidate(0, 0, 0, &predicate).unwrap());
    }
}
