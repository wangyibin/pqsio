//! Human-facing summaries and streaming text export; record I/O stays in Rust.
use crate::metadata::{obj, Value};
use crate::*;
use std::io::{BufWriter, Write};

const PAIRS: &[&str] = &[
    "readID", "chrom1", "pos1", "chrom2", "pos2", "strand1", "strand2", "mapq",
];
const CONCAT: &[&str] = &[
    "read_idx",
    "read_length",
    "read_start",
    "read_end",
    "strand",
    "chrom",
    "start",
    "end",
    "mapping_quality",
    "identity",
    "filter_reason",
];

pub fn info(path: impl AsRef<Path>, stats: bool) -> Result<Value> {
    let path = path.as_ref();
    progress::emit("Reading metadata and footers", 0, 0);
    let inspection = inspect(path)?;
    let kind = inspection.metadata.supported_kind()?;
    let mut bytes = 0u64;
    let mut q0 = 0u64;
    let mut q1 = 0u64;
    for shard in &inspection.shards {
        bytes = bytes
            .checked_add(fs::metadata(path.join(&shard.file))?.len())
            .context("size overflow")?;
        if shard.file.starts_with("q0/") {
            q0 += shard.records;
        } else {
            q1 += shard.records;
        }
    }
    let mut fields = vec![
        ("path", Value::from(path.to_string_lossy().as_ref())),
        (
            "format",
            Value::from(if kind == Kind::Pairs {
                "pairs"
            } else {
                "concat"
            }),
        ),
        (
            "format_version",
            Value::from(
                inspection
                    .metadata
                    .get_str("format-version")
                    .unwrap_or("unknown"),
            ),
        ),
        (
            "coordinates",
            Value::from(if kind == Kind::Pairs {
                "1-based"
            } else {
                "0-based half-open"
            }),
        ),
        ("q0_records", q0.into()),
        ("q1_records", q1.into()),
        ("nchroms", Value::from(inspection.contigs.len() as u64)),
        ("parquet_bytes", bytes.into()),
        ("shards", Value::from(inspection.shards.len() as u64)),
        ("declared_counts", Value::Object(inspection.declared_counts)),
        ("stats_scanned", Value::Bool(stats)),
    ];
    if stats {
        // q0 only: q1 is a redundant quality partition, not additional records.
        let mut reader = StreamingReader::open(path, 0, ReadOptions::default())?;
        let mut histogram = [0u64; 256];
        let (mut rows, mut reads, mut previous) = (0u64, 0u64, None);
        progress::emit("Scanning q0 MAPQ", 0, q0);
        while let Some(batch) = reader.next_batch()? {
            match batch {
                Batch::Pairs(batch) => {
                    for row in batch {
                        histogram[row.mapq as usize] += 1;
                        rows += 1;
                    }
                }
                Batch::Concat(batch) => {
                    for row in batch {
                        histogram[row.mapping_quality as usize] += 1;
                        rows += 1;
                        if previous != Some(row.read_idx) {
                            reads += 1;
                            previous = Some(row.read_idx);
                        }
                    }
                }
            }
            progress::emit("Scanning q0 MAPQ", rows, q0);
        }
        fields.push((
            "mapq_histogram",
            Value::List(histogram.into_iter().map(Value::from).collect()),
        ));
        fields.push(("scanned_records", rows.into()));
        if kind == Kind::Concat {
            fields.push(("scanned_reads", reads.into()));
        }
    }
    Ok(obj(fields))
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub auto_index: bool,
    pub format: String,
    pub columns: Option<Vec<String>>,
    pub limit: Option<u64>,
    pub header: bool,
    pub threads: usize,
    pub query: QueryOptions,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            auto_index: false,
            format: "auto".into(),
            columns: None,
            limit: None,
            header: true,
            threads: 1,
            query: QueryOptions::default(),
        }
    }
}

enum Source {
    Stream(Box<StreamingReader>),
    Query(Box<QueryReader>),
}
impl Source {
    fn next(&mut self) -> Result<Option<Batch>> {
        match self {
            Self::Stream(r) => r.next_batch(),
            Self::Query(r) => r.next_batch(),
        }
    }
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn safe_field(value: &str) -> Result<()> {
    ensure!(
        !value.chars().any(char::is_control),
        "text export cannot represent control characters in a field; omit the affected column"
    );
    Ok(())
}

/// Export to a new file, or stdout when output is None. Limits count rows, not
/// complete concat reads. Input order and stored endpoint orientation are kept.
pub fn export(
    input: impl AsRef<Path>,
    output: Option<&Path>,
    mut options: ExportOptions,
) -> Result<Value> {
    ensure!(options.threads > 0, "threads must be positive");
    if let Some(limit) = options.limit.filter(|n| *n > 0) {
        options.query.read.batch_rows = options
            .query
            .read
            .batch_rows
            .min(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    progress::emit("Opening PQS", 0, 0);
    let input = input.as_ref();
    ensure!(
        !options.query.regions.is_empty() || options.query.index == IndexMode::Auto,
        "index mode requires at least one region"
    );
    let (mut reader, kind, contigs) = if options.query.regions.is_empty() {
        let r = StreamingReader::open(input, options.query.min_mapq, options.query.read)?;
        ensure!(
            r.kind() == Kind::Pairs || options.query.pairs_mode == PairsMode::Either,
            "pairs_mode=both requires pairs"
        );
        let (kind, contigs) = (r.kind(), r.contigs().to_vec());
        (Source::Stream(Box::new(r)), kind, contigs)
    } else {
        let r = QueryReader::open(input, options.query.clone())?;
        let (kind, contigs) = (r.kind(), r.contigs().to_vec());
        (Source::Query(Box::new(r)), kind, contigs)
    };
    let format = match options.format.as_str() {
        "auto" if kind == Kind::Pairs && options.columns.is_none() => "pairs",
        "auto" if kind == Kind::Concat && options.columns.is_none() => "concat",
        "auto" | "tsv" => "tsv",
        "concat" => {
            ensure!(
                kind == Kind::Concat && options.columns.is_none(),
                "concat format requires concat input and all columns; use tsv for selected columns"
            );
            "concat"
        }
        "pairs" => {
            ensure!(kind == Kind::Pairs && options.columns.is_none(), "pairs format requires pairs input and all columns; use tsv for selected columns or concat");
            "pairs"
        }
        _ => bail!("format must be auto, pairs, concat or tsv"),
    };
    let schema = if kind == Kind::Pairs { PAIRS } else { CONCAT };
    let columns: Vec<&str> = options
        .columns
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_else(|| schema.to_vec());
    ensure!(!columns.is_empty(), "columns must not be empty");
    let mut seen = HashSet::new();
    let indices = columns
        .iter()
        .map(|name| {
            ensure!(seen.insert(*name), "duplicate column: {name}");
            schema
                .iter()
                .position(|s| s == name)
                .with_context(|| format!("unknown column {name}; available: {}", schema.join(",")))
        })
        .collect::<Result<Vec<_>>>()?;
    // Validate before opening any output or writing stdout headers.
    if options.header && format == "pairs" {
        for contig in &contigs {
            safe_field(&contig.name)?;
        }
    }
    if options.auto_index && options.query.index == IndexMode::Auto {
        if let Source::Query(r) = &reader {
            if query::build_missing_index(input, r.stats())? {
                reader = Source::Query(Box::new(QueryReader::open(input, options.query.clone())?));
            }
        }
    }
    let run = |writer: &mut dyn Write| -> Result<u64> {
        if options.header && format != "concat" {
            if format == "pairs" {
                writeln!(writer, "## pairs format v1.0\n#shape: whole matrix")?;
                for c in &contigs {
                    writeln!(writer, "#chromsize: {} {}", c.name, c.length)?;
                }
                writeln!(writer, "#columns: {}", columns.join(" "))?;
            } else {
                writeln!(writer, "{}", columns.join("\t"))?;
            }
        }
        let chrom = |id: u32| -> Result<String> {
            Ok(contigs
                .get(id as usize)
                .context("contig index out of range")?
                .name
                .clone())
        };
        let mut written = 0u64;
        progress::emit("Reading and exporting rows", 0, 0);
        while options.limit.is_none_or(|limit| written < limit) {
            let Some(batch) = reader.next()? else { break };
            let rows: Box<dyn Iterator<Item = Result<Vec<String>>>> = match batch {
                Batch::Pairs(rows) => Box::new(rows.into_iter().map(|r| {
                    Ok(vec![
                        r.read_id,
                        chrom(r.chrom1)?,
                        r.pos1.to_string(),
                        chrom(r.chrom2)?,
                        r.pos2.to_string(),
                        (r.strand1 as char).to_string(),
                        (r.strand2 as char).to_string(),
                        r.mapq.to_string(),
                    ])
                })),
                Batch::Concat(rows) => Box::new(rows.into_iter().map(|r| {
                    Ok(vec![
                        r.read_idx.to_string(),
                        r.read_length.to_string(),
                        r.read_start.to_string(),
                        r.read_end.to_string(),
                        (r.strand as char).to_string(),
                        chrom(r.chrom)?,
                        r.start.to_string(),
                        r.end.to_string(),
                        r.mapping_quality.to_string(),
                        r.identity.to_string(),
                        r.filter_reason,
                    ])
                })),
            };
            for row in rows {
                if options.limit.is_some_and(|limit| written >= limit) {
                    break;
                }
                let row = row?;
                for &i in &indices {
                    safe_field(&row[i])?;
                }
                for (n, &i) in indices.iter().enumerate() {
                    if n > 0 {
                        writer.write_all(b"\t")?;
                    }
                    writer.write_all(row[i].as_bytes())?;
                }
                writer.write_all(b"\n")?;
                written += 1;
            }
            progress::emit("Reading and exporting rows", written, 0);
        }
        Ok(written)
    };
    let mut run = run;
    let written = if let Some(output) = output {
        let absolute = std::path::absolute(output)?;
        let parent = absolute
            .parent()
            .context("output needs a parent")?
            .canonicalize()?;
        let target = parent.join(absolute.file_name().context("output needs a filename")?);
        ensure!(
            !target.starts_with(input.canonicalize()?),
            "export output must be outside the input PQS directory"
        );
        let mut name = target.as_os_str().to_os_string();
        name.push(".partial");
        let partial = PathBuf::from(name);
        for p in [&target, &partial] {
            match fs::symlink_metadata(p) {
                Ok(_) => bail!("output or staging path already exists: {}", p.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        let staging = Staging(partial);
        let compressed = matches!(
            target.extension().and_then(|s| s.to_str()),
            Some("gz" | "mgz")
        );
        let mut writer = io::writer_from_file(file, compressed, options.threads)?;
        let count = run(&mut writer)?;
        progress::emit("Finalizing text output", count, 0);
        writer.finish()?;
        fs::hard_link(&staging.0, &target)
            .context("cannot publish export without replacing existing output")?;
        count
    } else {
        let stdout = std::io::stdout();
        let mut writer = BufWriter::with_capacity(256 * 1024, stdout.lock());
        let count = run(&mut writer)?;
        writer.flush()?;
        count
    };
    let query_stats = match &reader {
        Source::Query(r) => Value::Object(Metadata::parse(&r.stats().to_json())?.fields),
        Source::Stream(_) => Value::Null,
    };
    Ok(obj([
        ("query_stats", query_stats),
        ("format", Value::from(format)),
        ("records", written.into()),
        (
            "output",
            output
                .map(|p| Value::from(p.to_string_lossy().as_ref()))
                .unwrap_or(Value::Null),
        ),
        (
            "columns",
            Value::List(columns.into_iter().map(Value::from).collect()),
        ),
    ]))
}

pub(crate) fn parse_options(text: &str) -> Result<ExportOptions> {
    let metadata = Metadata::parse(text)?;
    let mut options = ExportOptions::default();
    for (key, value) in metadata.fields {
        let number = || {
            value
                .u64()
                .with_context(|| format!("{key} must be an unsigned integer"))
        };
        match key.as_str() {
            "auto_index" => {
                let Value::Bool(enabled) = value else { bail!("auto_index must be bool") };
                options.auto_index = enabled;
            }
            "format" => options.format = value.string().context("format must be a string")?.into(),
            "columns" if value == Value::Null => (),
            "columns" => {
                let Value::List(items) = value else {
                    bail!("columns must be a list")
                };
                options.columns = Some(
                    items
                        .iter()
                        .map(|v| {
                            v.string()
                                .map(str::to_owned)
                                .context("column must be a string")
                        })
                        .collect::<Result<_>>()?,
                );
            }
            "limit" if value == Value::Null => (),
            "limit" => options.limit = Some(number()?),
            "header" => {
                let Value::Bool(b) = value else {
                    bail!("header must be bool")
                };
                options.header = b;
            }
            "threads" => options.threads = usize::try_from(number()?)?,
            "batch_rows" => options.query.read.batch_rows = usize::try_from(number()?)?,
            "min_mapq" => options.query.min_mapq = u8::try_from(number()?)?,
            "index" => {
                options.query.index = match value.string() {
                    Some("auto") => IndexMode::Auto,
                    Some("off") => IndexMode::Off,
                    Some("require") => IndexMode::Require,
                    _ => bail!("index must be auto, off or require"),
                }
            }
            "filter_mode" if value == Value::Null => (),
            "filter_mode" => {
                options.query.read.concat_filter = Some(match value.string() {
                    Some("matching_alignments") => ConcatFilter::MatchingAlignments,
                    Some("complete_reads") => ConcatFilter::CompleteReads,
                    _ => bail!("filter_mode must be matching_alignments or complete_reads"),
                })
            }
            "pairs_mode" => {
                options.query.pairs_mode = match value.string() {
                    Some("either") => PairsMode::Either,
                    Some("both") => PairsMode::Both,
                    _ => bail!("pairs_mode must be either or both"),
                }
            }
            "regions" => {
                let Value::List(items) = value else {
                    bail!("regions must be a list")
                };
                for item in items {
                    let Value::List(v) = item else {
                        bail!("region must be [chrom, start, end]")
                    };
                    ensure!(v.len() == 3, "region must be [chrom, start, end]");
                    options.query.regions.push(Region {
                        contig: v[0]
                            .string()
                            .context("region chrom must be a string")?
                            .into(),
                        start: v[1].u64().context("region start must be unsigned")?,
                        end: v[2].u64().context("region end must be unsigned")?,
                    });
                }
            }
            _ => bail!("unknown export option: {key}"),
        }
    }
    Ok(options)
}
