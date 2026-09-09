//! Read-only inspection and bounded row-group validation. No repair side effects.
use crate::metadata::{obj, Value};
use crate::*;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: &'static str,
    pub code: &'static str,
    pub file: String,
    /// One-based row within the original shard; None for file/schema issues.
    pub row: Option<u64>,
    pub field: Option<String>,
    pub message: String,
}
impl Issue {
    fn value(&self) -> Value {
        obj([
            ("severity", Value::from(self.severity)),
            ("code", Value::from(self.code)),
            ("file", Value::from(self.file.as_str())),
            ("row", self.row.map(Value::from).unwrap_or(Value::Null)),
            (
                "field",
                self.field
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("message", Value::from(self.message.as_str())),
        ])
    }
}
#[derive(Clone, Debug)]
pub struct ShardInfo {
    pub file: String,
    pub records: u64,
    pub row_groups: usize,
    pub schema: BTreeMap<String, Value>,
}
#[derive(Clone, Debug)]
pub struct Inspection {
    pub metadata: Metadata,
    pub contigs: Vec<Contig>,
    pub declared_counts: BTreeMap<String, Value>,
    pub shards: Vec<ShardInfo>,
    pub copy_numbers: Value,
}
impl Inspection {
    pub fn to_value(&self) -> Value {
        let coordinates = match self.metadata.supported_kind() {
            Ok(Kind::Pairs) => "1-based",
            Ok(Kind::Concat) => "0-based half-open",
            _ => "unknown",
        };
        obj([
            ("metadata", self.metadata.to_value()),
            ("copy_numbers", self.copy_numbers.clone()),
            ("coordinates", Value::from(coordinates)),
            (
                "read_idx_scope",
                self.metadata
                    .fields
                    .get("read_idx_scope")
                    .cloned()
                    .unwrap_or_else(|| Value::from("global")),
            ),
            (
                "contigs",
                Value::List(
                    self.contigs
                        .iter()
                        .map(|c| {
                            obj([
                                ("name", Value::from(c.name.as_str())),
                                ("length", Value::from(c.length)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "declared_counts",
                Value::Object(self.declared_counts.clone()),
            ),
            (
                "observed",
                obj([
                    (
                        "shards",
                        Value::List(
                            self.shards
                                .iter()
                                .map(|s| {
                                    obj([
                                        ("file", Value::from(s.file.as_str())),
                                        ("records", Value::from(s.records)),
                                        ("row_groups", Value::from(s.row_groups as u64)),
                                        ("schema", Value::Object(s.schema.clone())),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "records",
                        Value::Object(
                            ["q0", "q1"]
                                .into_iter()
                                .map(|q| {
                                    (
                                        q.into(),
                                        Value::from(
                                            self.shards
                                                .iter()
                                                .filter(|s| s.file.starts_with(&format!("{q}/")))
                                                .map(|s| s.records)
                                                .sum::<u64>(),
                                        ),
                                    )
                                })
                                .collect(),
                        ),
                    ),
                ]),
            ),
            (
                "checks_not_performed",
                Value::List(
                    ["record_values", "read_groups", "q0_q1_consistency"]
                        .into_iter()
                        .map(Value::from)
                        .collect(),
                ),
            ),
        ])
    }
    pub fn to_json(&self) -> String {
        self.to_value().json()
    }
}
fn files(path: &Path, q: &str) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(path.join(q))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "parquet"));
    paths.sort_by_key(|p| {
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        (
            name.parse::<u64>().ok().is_none(),
            name.parse::<u64>().unwrap_or(0),
            name,
        )
    });
    Ok(paths)
}
pub fn inspect(path: impl AsRef<Path>) -> Result<Inspection> {
    let path = path.as_ref();
    let metadata = Metadata::open(path).context("_metadata")?;
    let mut contigs = vec![];
    let mut names = HashSet::new();
    for (i, line) in fs::read_to_string(path.join("_contigsizes"))?
        .lines()
        .enumerate()
    {
        let f: Vec<_> = line.split_whitespace().collect();
        ensure!(f.len() == 2, "invalid _contigsizes line {}", i + 1);
        ensure!(
            names.insert(f[0].to_owned()),
            "duplicate contig at line {}",
            i + 1
        );
        contigs.push(Contig {
            name: f[0].into(),
            length: f[1]
                .parse()
                .with_context(|| format!("invalid contig length at line {}", i + 1))?,
        });
    }
    let mut declared_counts = BTreeMap::new();
    match fs::read_to_string(path.join("_metadata_counts")) {
        Ok(text) => {
            for line in text.lines() {
                let f: Vec<_> = line.split_whitespace().collect();
                ensure!(f.len() == 2, "invalid _metadata_counts line");
                ensure!(
                    declared_counts
                        .insert(
                            f[0].into(),
                            Value::from(f[1].parse::<u64>().context("invalid declared count")?)
                        )
                        .is_none(),
                    "duplicate count key"
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut shards = vec![];
    for q in ["q0", "q1"] {
        for file in files(path, q)? {
            let mut r = ParquetReader::new(File::open(&file)?);
            let meta = r.get_metadata()?.clone();
            let schema = r.schema()?;
            shards.push(ShardInfo {
                file: format!("{q}/{}", file.file_name().unwrap().to_string_lossy()),
                records: meta.num_rows as u64,
                row_groups: meta.row_groups.len(),
                schema: schema
                    .iter()
                    .map(|(n, d)| {
                        (
                            n.to_string(),
                            Value::from(dtype(&DataType::from_arrow_field(d))),
                        )
                    })
                    .collect(),
            });
        }
    }
    let copy_numbers = match copy_numbers::read_with_contigs(path, &contigs) {
        Ok(cn) => cn.to_value(),
        Err(e) => obj([
            ("status", Value::from(if e.downcast_ref::<copy_numbers::CopyNumberError>().is_some() { "invalid" } else { "unreadable" })),
            ("present", match fs::symlink_metadata(path.join("cn.info")) { Ok(_) => Value::Bool(true), Err(_) => Value::Null }),
            ("error", Value::from(format!("{e:#}").as_str())),
        ]),
    };
    Ok(Inspection {
        copy_numbers,
        metadata,
        contigs,
        declared_counts,
        shards,
    })
}
fn dtype(d: &DataType) -> &'static str {
    match d {
        DataType::String => "String",
        DataType::Categorical(_, _) => "Categorical",
        DataType::UInt8 => "UInt8",
        DataType::UInt16 => "UInt16",
        DataType::UInt32 => "UInt32",
        DataType::UInt64 => "UInt64",
        DataType::Int8 => "Int8",
        DataType::Int16 => "Int16",
        DataType::Int32 => "Int32",
        DataType::Int64 => "Int64",
        DataType::Float32 => "Float32",
        DataType::Float64 => "Float64",
        DataType::Boolean => "Boolean",
        _ => "unsupported",
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationLevel {
    Quick,
    Full,
}
#[derive(Debug)]
pub struct ValidationReport {
    pub status: &'static str,
    pub level: ValidationLevel,
    pub issues: Vec<Issue>,
    pub total_issues: usize,
    pub checks_completed: Vec<&'static str>,
    pub checks_skipped: Vec<&'static str>,
    pub inspection: Option<Inspection>,
    max_issues: usize,
}
impl ValidationReport {
    fn issue(
        &mut self,
        severity: &'static str,
        code: &'static str,
        file: &str,
        row: Option<u64>,
        field: Option<&str>,
        message: impl Into<String>,
    ) {
        self.total_issues += 1;
        if severity == "error" {
            self.status = "invalid";
        }
        if self.issues.len() < self.max_issues {
            self.issues.push(Issue {
                severity,
                code,
                file: file.into(),
                row,
                field: field.map(str::to_owned),
                message: message.into(),
            });
        }
    }
    fn incomplete(&mut self, code: &'static str, file: &str, message: impl Into<String>) {
        if self.status != "invalid" {
            self.status = "incomplete";
        }
        self.issue("warning", code, file, None, None, message);
    }
    pub fn to_value(&self) -> Value {
        obj([
            ("status", Value::from(self.status)),
            (
                "level",
                Value::from(if self.level == ValidationLevel::Quick {
                    "quick"
                } else {
                    "full"
                }),
            ),
            (
                "issues",
                Value::List(self.issues.iter().map(Issue::value).collect()),
            ),
            ("total_issues", Value::from(self.total_issues as u64)),
            (
                "issues_truncated",
                Value::Bool(self.total_issues > self.issues.len()),
            ),
            (
                "checks_completed",
                Value::List(
                    self.checks_completed
                        .iter()
                        .copied()
                        .map(Value::from)
                        .collect(),
                ),
            ),
            (
                "checks_skipped",
                Value::List(
                    self.checks_skipped
                        .iter()
                        .copied()
                        .map(Value::from)
                        .collect(),
                ),
            ),
            (
                "inspection",
                self.inspection
                    .as_ref()
                    .map(Inspection::to_value)
                    .unwrap_or(Value::Null),
            ),
        ])
    }
    pub fn to_json(&self) -> String {
        self.to_value().json()
    }
}

/// `max_issues` caps stored examples, not scanning or total issue counts.
pub fn validate(
    path: impl AsRef<Path>,
    level: ValidationLevel,
    max_issues: usize,
) -> Result<ValidationReport> {
    ensure!(max_issues > 0, "max_issues must be positive");
    let path = path.as_ref();
    let mut report = ValidationReport {
        status: "valid",
        level,
        issues: vec![],
        total_issues: 0,
        checks_completed: vec![],
        checks_skipped: vec![],
        inspection: None,
        max_issues,
    };
    if !path.is_dir() {
        report.incomplete(
            "DATASET_UNAVAILABLE",
            "",
            "dataset directory does not exist or cannot be accessed",
        );
        report.checks_skipped.extend([
            "metadata",
            "schema",
            "record_values",
            "read_groups",
            "q0_q1_consistency",
        ]);
        return Ok(report);
    }
    match read_copy_numbers(path) {
        Ok(_) => {},
        Err(e) => {
            if let Some(cn) = e.downcast_ref::<copy_numbers::CopyNumberError>() {
                report.issue("error", cn.code, &cn.file, Some(cn.line), Some(&cn.contig), cn.to_string());
            } else { report.incomplete("CN_UNREADABLE", "cn.info", format!("{e:#}")); }
        }
    }
    for name in [
        "_metadata",
        "_contigsizes",
        "_metadata_counts",
        "_readme",
        "q0",
        "q1",
    ] {
        let p = path.join(name);
        if !(if name.starts_with('q') {
            p.is_dir()
        } else {
            p.is_file()
        }) {
            report.issue(
                "error",
                "MISSING_COMPONENT",
                name,
                None,
                None,
                "required file or directory missing",
            );
        }
    }
    let info = match inspect(path) {
        Ok(info) => info,
        Err(e) => {
            if e.downcast_ref::<std::io::Error>().is_none() {
                report.issue(
                    "error",
                    "MALFORMED_DATASET",
                    "",
                    None,
                    None,
                    format!("{e:#}"),
                );
            }
            report.incomplete("INSPECTION_FAILED", "", format!("{e:#}"));
            report.checks_skipped.extend([
                "metadata",
                "schema",
                "record_values",
                "read_groups",
                "q0_q1_consistency",
            ]);
            return Ok(report);
        }
    };
    for name in ["format", "format-version"] {
        if info.metadata.get_str(name).is_none() {
            report.issue(
                "error",
                "INVALID_METADATA_FIELD",
                "_metadata",
                None,
                Some(name),
                "required string field missing or has wrong type",
            );
        }
    }
    if info.metadata.fields.get("is_pqs") != Some(&Value::Bool(true)) {
        report.issue(
            "error",
            "INVALID_METADATA_FIELD",
            "_metadata",
            None,
            Some("is_pqs"),
            "is_pqs must be True",
        );
    }
    if let Some(value) = info.metadata.fields.get("q1_min_mapq") {
        if value.u64() != Some(1) {
            report.issue(
                "error",
                "INVALID_METADATA_FIELD",
                "_metadata",
                None,
                Some("q1_min_mapq"),
                "supported q1 semantics require MAPQ >= 1",
            );
        }
    }
    let kind = match info.metadata.supported_kind() {
        Ok(kind) => kind,
        Err(e) => {
            report.incomplete("UNSUPPORTED_FORMAT", "_metadata", e.to_string());
            report.checks_skipped.extend([
                "schema",
                "record_values",
                "read_groups",
                "q0_q1_consistency",
            ]);
            report.inspection = Some(info);
            return Ok(report);
        }
    };
    if info.contigs.is_empty() || info.contigs.iter().any(|c| c.length == 0) {
        report.issue(
            "error",
            "INVALID_CONTIGS",
            "_contigsizes",
            None,
            None,
            "at least one contig and positive lengths required",
        );
    }
    if let Some(scope) = info.metadata.fields.get("read_idx_scope") {
        if !matches!(scope.string(), Some("shard" | "global")) {
            report.incomplete(
                "UNSUPPORTED_ID_SCOPE",
                "_metadata",
                "read_idx_scope must be global or shard",
            );
            report
                .checks_skipped
                .extend(["record_values", "read_groups", "q0_q1_consistency"]);
            report.inspection = Some(info);
            return Ok(report);
        }
    }
    if info
        .metadata
        .fields
        .get("chunksize")
        .and_then(Value::u64)
        .is_none_or(|n| n == 0)
    {
        report.issue(
            "error",
            "INVALID_CHUNKSIZE",
            "_metadata",
            None,
            Some("chunksize"),
            "positive integer chunksize required",
        );
    }
    // Use the writer's canonical schema declaration, including long pairs positions.
    let expected = Metadata::parse(&crate::metadata(kind, 1, &info.contigs))?;
    let schema = expected.fields["schema"].object().unwrap();
    let declared_schema = info.metadata.fields.get("schema").and_then(Value::object);
    if declared_schema.is_none() {
        report.issue(
            "error",
            "MISSING_SCHEMA",
            "_metadata",
            None,
            Some("schema"),
            "schema dictionary required",
        );
    }
    if info.metadata.fields.get("columns") != expected.fields.get("columns") {
        report.issue(
            "error",
            "COLUMNS_MISMATCH",
            "_metadata",
            None,
            Some("columns"),
            "column names/order differ from supported format",
        );
    }
    for (name, want) in schema {
        let declared = declared_schema.and_then(|s| s.get(name));
        let compatible = declared == Some(want)
            || (kind == Kind::Pairs
                && (name == "pos1" || name == "pos2")
                && declared.and_then(Value::string) == Some("UInt64"));
        if !compatible {
            report.issue(
                "error",
                "INVALID_DECLARED_SCHEMA",
                "_metadata",
                None,
                Some(name),
                "declared field type differs from supported schema",
            );
        }
    }
    let declared_dtypes = info.metadata.fields.get("dtypes").and_then(Value::object);
    for (name, want) in expected.fields["dtypes"].object().unwrap() {
        let declared = declared_dtypes.and_then(|d| d.get(name));
        let compatible = declared == Some(want)
            || (kind == Kind::Pairs
                && (name == "pos1" || name == "pos2")
                && declared.and_then(Value::string) == Some("uint64"));
        if !compatible {
            report.issue(
                "error",
                "INVALID_DECLARED_DTYPE",
                "_metadata",
                None,
                Some(name),
                "declared dtype differs from supported format",
            );
        }
    }
    for shard in &info.shards {
        for (name, want) in schema {
            let actual = shard.schema.get(name);
            // UInt64 pairs positions are valid even when UInt32 would suffice.
            let compatible = actual == Some(want)
                || (kind == Kind::Pairs
                    && (name == "pos1" || name == "pos2")
                    && actual.and_then(Value::string) == Some("UInt64"));
            if !compatible {
                report.issue(
                    "error",
                    "SCHEMA_MISMATCH",
                    &shard.file,
                    None,
                    Some(name),
                    format!("expected {}, observed {:?}", want.json(), actual),
                );
            }
            if let Some(declared) = declared_schema.and_then(|s| s.get(name)) {
                if Some(declared) != actual {
                    report.issue(
                        "error",
                        "DECLARED_SCHEMA_MISMATCH",
                        &shard.file,
                        None,
                        Some(name),
                        "footer type differs from declared schema",
                    );
                }
            } else {
                report.issue(
                    "error",
                    "MISSING_DECLARED_FIELD",
                    "_metadata",
                    None,
                    Some(name),
                    "field missing from schema",
                );
            }
        }
    }
    for q in ["q0", "q1"] {
        let key = format!("{q}_records");
        let observed: u64 = info
            .shards
            .iter()
            .filter(|s| s.file.starts_with(&format!("{q}/")))
            .map(|s| s.records)
            .sum();
        match info.declared_counts.get(&key).and_then(Value::u64) {
            Some(n) if n != observed => report.issue(
                "error",
                "COUNT_MISMATCH",
                "_metadata_counts",
                None,
                Some(&key),
                format!("declared {n}, observed {observed}"),
            ),
            None => report.issue(
                "warning",
                "COUNT_NOT_DECLARED",
                "_metadata_counts",
                None,
                Some(&key),
                "count is absent, not assumed zero",
            ),
            _ => {}
        }
    }
    report
        .checks_completed
        .extend(["metadata", "schema", "footer_counts"]);
    if level == ValidationLevel::Full {
        match full(path, &info, &mut report) {
            Ok(()) => {}
            Err(e) => {
                report.incomplete("SCAN_FAILED", "", format!("{e:#}"));
                for name in ["record_values", "read_groups", "q0_q1_consistency"] {
                    if !report.checks_completed.contains(&name)
                        && !report.checks_skipped.contains(&name)
                    {
                        report.checks_skipped.push(name);
                    }
                }
            }
        }
    } else {
        report
            .checks_skipped
            .extend(["record_values", "read_groups", "q0_q1_consistency"]);
    }
    report.inspection = Some(info);
    Ok(report)
}

#[derive(Debug, PartialEq)]
enum Record {
    Pair(Pair),
    Concat(Alignment),
}
impl Record {
    fn mapq(&self) -> u8 {
        match self {
            Self::Pair(r) => r.mapq,
            Self::Concat(r) => r.mapping_quality,
        }
    }
}
struct Raw {
    source: Reader,
    files: std::vec::IntoIter<PathBuf>,
    decoder: Option<(File, ParquetReader<File>, usize)>,
    rows: std::vec::IntoIter<Record>,
    file: String,
    row: u64,
}
impl Raw {
    fn open(path: &Path, q: &str) -> Result<Self> {
        Ok(Self {
            source: Reader::open(path, 0)?,
            files: files(path, q)?.into_iter(),
            decoder: None,
            rows: vec![].into_iter(),
            file: String::new(),
            row: 0,
        })
    }
    fn next(&mut self, report: &mut ValidationReport) -> Result<Option<Record>> {
        loop {
            if let Some(r) = self.rows.next() {
                self.row += 1;
                return Ok(Some(r));
            }
            if self.decoder.is_none() {
                let Some(path) = self.files.next() else {
                    return Ok(None);
                };
                self.file = format!(
                    "{}/{}",
                    path.parent()
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_string_lossy(),
                    path.file_name().unwrap().to_string_lossy()
                );
                self.row = 0;
                let f = File::open(path)?;
                self.decoder = Some((f.try_clone()?, ParquetReader::new(f), 0));
            }
            let (f, r, index) = self.decoder.as_mut().unwrap();
            let meta = r.get_metadata().with_context(|| self.file.clone())?;
            if *index == meta.row_groups.len() {
                self.decoder = None;
                continue;
            }
            let group = meta.row_groups[*index].clone();
            *index += 1;
            let mut one = meta.as_ref().clone();
            one.num_rows = group.num_rows();
            one.row_groups = vec![group];
            let mut reader =
                ParquetReader::new(f.try_clone()?).read_parallel(ParallelStrategy::None);
            reader.set_metadata(Arc::new(one));
            let frame = reader.finish().map_err(|e| {
                report.issue(
                    "error",
                    "PARQUET_DECODE_FAILED",
                    &self.file,
                    None,
                    None,
                    format!("row group could not be decoded: {e}"),
                );
                e
            })?;
            for col in frame.get_columns() {
                if col.null_count() != 0 {
                    report.issue(
                        "error",
                        "NULL_VALUES",
                        &self.file,
                        None,
                        Some(col.name().as_str()),
                        "null values in row group; remaining scan could not be completed",
                    );
                    bail!(
                        "{} after row {}: null values in {}",
                        self.file,
                        self.row,
                        col.name()
                    );
                }
            }
            // Raw IDs, no filters/remapping; schema already inspected separately.
            let batch = self.source.frame_batch(frame).map_err(|e| {
                report.issue(
                    "error",
                    "INVALID_RECORD_ENCODING",
                    &self.file,
                    None,
                    None,
                    format!("after row {}: {e:#}", self.row),
                );
                e
            })?;
            self.rows = match batch {
                Batch::Pairs(v) => v
                    .into_iter()
                    .map(Record::Pair)
                    .collect::<Vec<_>>()
                    .into_iter(),
                Batch::Concat(v) => v
                    .into_iter()
                    .map(Record::Concat)
                    .collect::<Vec<_>>()
                    .into_iter(),
            };
        }
    }
}
#[derive(Default)]
struct Group {
    previous: Option<(String, u64, u32)>,
    count: u64,
    records: u64,
}
fn check(
    r: &Record,
    raw: &Raw,
    info: &Inspection,
    state: &mut Group,
    report: &mut ValidationReport,
) {
    state.records += 1;
    let mut issue = |code, field, message: &str| {
        report.issue(
            "error",
            code,
            &raw.file,
            Some(raw.row),
            Some(field),
            message,
        )
    };
    let length = |id: u32| info.contigs.get(id as usize).map(|c| c.length).unwrap_or(0);
    match r {
        Record::Pair(r) => {
            for (name, pos, chrom) in [("pos1", r.pos1, r.chrom1), ("pos2", r.pos2, r.chrom2)] {
                if pos == 0 || pos > length(chrom) {
                    issue(
                        "POSITION_OUT_OF_RANGE",
                        name,
                        "pairs position must be in 1..=contig length",
                    );
                }
            }
        }
        Record::Concat(r) => {
            if r.start >= r.end || r.end > length(r.chrom) {
                issue(
                    "POSITION_OUT_OF_RANGE",
                    "end",
                    "reference interval must satisfy start < end <= contig length",
                );
            }
            if r.read_start >= r.read_end || r.read_end > r.read_length {
                issue(
                    "INVALID_READ_INTERVAL",
                    "read_end",
                    "query interval must satisfy start < end <= read length",
                );
            }
            if !r.identity.is_finite() {
                issue("INVALID_IDENTITY", "identity", "identity must be finite");
            }
            let scope = if info.metadata.shard_scoped() {
                raw.file.clone()
            } else {
                String::new()
            };
            if let Some((s, id, len)) = &state.previous {
                if *s == scope && r.read_idx < *id {
                    issue(
                        "READ_ORDER",
                        "read_idx",
                        "read IDs decrease or a completed read reappears",
                    );
                }
                if *s == scope && r.read_idx == *id && r.read_length != *len {
                    issue(
                        "READ_LENGTH_MISMATCH",
                        "read_length",
                        "read length changed within a read",
                    );
                }
            }
            if state
                .previous
                .as_ref()
                .is_none_or(|(s, id, _)| *s != scope || *id != r.read_idx)
            {
                state.count += 1;
            }
            state.previous = Some((scope, r.read_idx, r.read_length));
        }
    }
    if raw.file.starts_with("q1/") && r.mapq() == 0 {
        issue("Q1_MAPQ", "mapq", "q1 contains MAPQ zero record");
    }
}
fn full(path: &Path, info: &Inspection, report: &mut ValidationReport) -> Result<()> {
    let mut q0 = Raw::open(path, "q0")?;
    let mut q1 = Raw::open(path, "q1")?;
    let mut g0 = Group::default();
    let mut g1 = Group::default();
    let mut matches = true;
    while let Some(r) = q0.next(report)? {
        check(&r, &q0, info, &mut g0, report);
        if r.mapq() > 0 {
            let other = q1.next(report)?;
            if let Some(ref other) = other {
                check(other, &q1, info, &mut g1, report);
            }
            // Local IDs only carry meaning within their original named shard.
            let same_scope = !info.metadata.shard_scoped()
                || q0.file.strip_prefix("q0/") == q1.file.strip_prefix("q1/");
            matches &= other.as_ref() == Some(&r) && same_scope;
        }
    }
    while let Some(r) = q1.next(report)? {
        matches = false;
        check(&r, &q1, info, &mut g1, report);
    }
    if matches {
        report.checks_completed.push("q0_q1_consistency");
    } else {
        report.incomplete("Q0_Q1_SEQUENCE_MISMATCH", "q1", "q1 does not equal the ordered MAPQ>=1 projection of q0; content, order or local-ID shard correspondence differs. No unordered multiset proof was performed");
        report.checks_skipped.push("q0_q1_consistency");
    }
    for (key, observed) in [("q0_records", g0.records), ("q1_records", g1.records)] {
        if let Some(declared) = info.declared_counts.get(key).and_then(Value::u64) {
            if declared != observed {
                report.issue(
                    "error",
                    "SCANNED_COUNT_MISMATCH",
                    "_metadata_counts",
                    None,
                    Some(key),
                    format!("declared {declared}, scanned {observed}"),
                );
            }
        }
    }
    if info.metadata.kind()? == Kind::Concat {
        for (key, observed) in [("q0_concats", g0.count), ("q1_concats", g1.count)] {
            match info.declared_counts.get(key).and_then(Value::u64) {
                Some(n) if n != observed => report.issue(
                    "error",
                    "COUNT_MISMATCH",
                    "_metadata_counts",
                    None,
                    Some(key),
                    format!("declared {n}, observed {observed}"),
                ),
                None => report.issue(
                    "warning",
                    "COUNT_NOT_DECLARED",
                    "_metadata_counts",
                    None,
                    Some(key),
                    "read count not declared",
                ),
                _ => {}
            }
        }
    }
    report
        .checks_completed
        .extend(["record_values", "read_groups"]);
    Ok(())
}
