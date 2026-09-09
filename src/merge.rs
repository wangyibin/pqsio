//! Ordered q0-only streaming merge. See docs/merge.md for guarantees and limits.
use crate::metadata::{obj, Value};
use crate::*;
use std::io::{BufWriter, Write};

#[derive(Clone, Copy, Debug)]
pub struct MergeOptions {
    pub chunk_size: usize,
    pub batch_rows: usize,
    pub provenance: bool,
}
impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            chunk_size: 1_000_000,
            batch_rows: 65_536,
            provenance: true,
        }
    }
}
#[derive(Clone, Debug)]
pub struct MergeSource {
    pub index: usize,
    pub path: PathBuf,
    pub format_version: String,
    /// None means the legacy, implicit global interpretation.
    pub read_id_scope: Option<String>,
    pub input_records: u64,
    pub output_records: u64,
    pub output_reads: u64,
    pub omitted_sidecars: Vec<String>,
    pub copy_numbers_present: bool,
}
impl MergeSource {
    pub fn to_value(&self) -> Value {
        obj([
            ("schema_version", Value::from(1)),
            ("copy_numbers_propagated", Value::Bool(self.copy_numbers_present)),
            ("source_index", Value::from(self.index as u64)),
            (
                "path_label",
                Value::from(self.path.to_string_lossy().as_ref()),
            ),
            ("format_version", Value::from(self.format_version.as_str())),
            (
                "input_read_id_scope",
                self.read_id_scope
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            (
                "input_records",
                Value::from(self.input_records.to_string().as_str()),
            ),
            (
                "output_records",
                Value::from(self.output_records.to_string().as_str()),
            ),
            (
                "output_reads",
                Value::from(self.output_reads.to_string().as_str()),
            ),
            (
                "omitted_sidecars",
                Value::List(
                    self.omitted_sidecars
                        .iter()
                        .map(|s| Value::from(s.as_str()))
                        .collect(),
                ),
            ),
        ])
    }
}
#[derive(Clone, Debug)]
pub struct MergeResult {
    pub output: PathBuf,
    pub kind: Kind,
    pub counts: Counts,
    pub sources: Vec<MergeSource>,
    pub provenance: bool,
}
impl MergeResult {
    pub fn to_value(&self) -> Value {
        obj([
            (
                "output",
                Value::from(self.output.to_string_lossy().as_ref()),
            ),
            (
                "format",
                Value::from(if self.kind == Kind::Pairs {
                    "pairs"
                } else {
                    "concat"
                }),
            ),
            (
                "counts",
                obj([
                    ("q0_records", Value::from(self.counts.q0_records)),
                    ("q1_records", Value::from(self.counts.q1_records)),
                    ("q0_concats", Value::from(self.counts.q0_concats)),
                    ("q1_concats", Value::from(self.counts.q1_concats)),
                ]),
            ),
            (
                "sources",
                Value::List(self.sources.iter().map(MergeSource::to_value).collect()),
            ),
            (
                "provenance_files",
                Value::List(if self.provenance {
                    let mut names = vec![Value::from("_merge_sources.jsonl")];
                    if self.kind == Kind::Concat {
                        names.push(Value::from("_merge_reads.jsonl"));
                    }
                    names
                } else {
                    vec![]
                }),
            ),
        ])
    }
    pub fn to_json(&self) -> String {
        self.to_value().json()
    }
}
struct Input {
    canonical: PathBuf,
    map: Vec<u32>,
    source: MergeSource,
}
fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => bail!("output or staging path already exists: {}", path.display()),
    }
}
fn remap(ids: &mut [u32], map: &[u32]) -> Result<()> {
    for id in ids {
        *id = *map
            .get(*id as usize)
            .context("input contig ID out of range")?;
    }
    Ok(())
}
fn increment(value: &mut u64, amount: u64) -> Result<()> {
    *value = value
        .checked_add(amount)
        .context("merge ID or record count overflow")?;
    Ok(())
}

/// Merge caller-ordered inputs through the synchronous column Writer.
/// Inputs must remain unchanged until this function returns.
pub fn merge<P: AsRef<Path>>(
    inputs: &[P],
    output: impl AsRef<Path>,
    options: MergeOptions,
) -> Result<MergeResult> {
    ensure!(!inputs.is_empty(), "merge requires at least one input");
    ensure!(options.chunk_size > 0, "chunk_size must be positive");
    ensure!(
        options.batch_rows > 0 && options.batch_rows <= u32::MAX as usize,
        "batch_rows must be in 1..=4294967295"
    );
    let output = output.as_ref();
    absent(output)?;
    let leaf = output
        .file_name()
        .context("output must name a new dataset directory")?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let target = fs::canonicalize(parent)
        .context("output parent must exist and be readable")?
        .join(leaf);
    absent(&target)?;
    let mut partial = target.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    absent(&partial)?;
    let mut seen = HashSet::new();
    let mut prepared = Vec::new();
    let mut contigs: Vec<Contig> = vec![];
    let mut names: HashMap<String, (u32, PathBuf)> = HashMap::new();
    let mut kind = None;
    for (index, path) in inputs.iter().enumerate() {
        let path = path.as_ref();
        let input = (|| -> Result<Input> {
            let canonical = fs::canonicalize(path)?;
            ensure!(
                seen.insert(canonical.clone()),
                "duplicate input (including path aliases): {}",
                path.display()
            );
            for dest in [&target, &partial] {
                ensure!(
                    !dest.starts_with(&canonical) && !canonical.starts_with(dest),
                    "output/staging overlaps input {}",
                    path.display()
                );
            }
            for directory in ["q0", "q1"] {
                let resolved = fs::canonicalize(canonical.join(directory))?;
                for dest in [&target, &partial] {
                    ensure!(
                        !dest.starts_with(&resolved) && !resolved.starts_with(dest),
                        "output/staging overlaps input {} directory {}",
                        path.display(),
                        directory
                    );
                }
            }
            let metadata = Metadata::open(&canonical)?;
            let input_kind = metadata.supported_kind()?;
            ensure!(
                kind.is_none_or(|k| k == input_kind),
                "cannot merge pairs and concat together"
            );
            kind = Some(input_kind);
            if let Some(scope) = metadata.fields.get("read_idx_scope") {
                ensure!(
                    matches!(scope.string(), Some("global" | "shard")),
                    "unsupported read_idx_scope"
                );
            }
            if let Some(value) = metadata.fields.get("is_with_mapq") {
                ensure!(
                    value == &Value::Bool(true),
                    "unsupported is_with_mapq semantics"
                );
            }
            if input_kind == Kind::Concat {
                for (key, expected) in [
                    ("record_type", "porec_alignment"),
                    ("partition_key", "read_idx"),
                ] {
                    if let Some(value) = metadata.fields.get(key) {
                        ensure!(
                            value.string() == Some(expected),
                            "unsupported {key} semantics"
                        );
                    }
                }
            }
            // Reuse the legacy contig parser and deterministic shard discovery,
            // without opening Parquet footers or scanning records.
            let reader = Reader::open(&canonical, 0)?;
            for entry in fs::read_dir(canonical.join("q1"))? {
                entry?;
            }
            ensure!(
                !reader.contigs.is_empty(),
                "at least one contig is required"
            );
            let mut map = vec![];
            for c in &reader.contigs {
                ensure!(c.length > 0, "zero length contig {}", c.name);
                if let Some((id, first)) = names.get(&c.name) {
                    let length = contigs[*id as usize].length;
                    ensure!(
                        length == c.length,
                        "contig {} length conflict: {} declares {}, {} declares {}",
                        c.name,
                        first.display(),
                        length,
                        path.display(),
                        c.length
                    );
                    map.push(*id);
                } else {
                    let id = u32::try_from(contigs.len()).context("too many output contigs")?;
                    names.insert(c.name.clone(), (id, path.to_path_buf()));
                    contigs.push(c.clone());
                    map.push(id);
                }
            }
            let mut omitted_sidecars = vec![];
            for entry in fs::read_dir(&canonical)? {
                let name = entry?.file_name().to_string_lossy().into_owned();
                if !matches!(
                    name.as_str(),
                    "q0" | "q1" | "_metadata" | "_metadata_counts" | "_contigsizes" | "_readme" | "cn.info"
                ) {
                    omitted_sidecars.push(name);
                }
            }
            omitted_sidecars.sort();
            Ok(Input {
                canonical,
                map,
                source: MergeSource {
                    index,
                    path: path.into(),
                    format_version: metadata.get_str("format-version").unwrap().into(),
                    read_id_scope: metadata.get_str("read_idx_scope").map(str::to_owned),
                    input_records: 0,
                    output_records: 0,
                    output_reads: 0,
                    copy_numbers_present: fs::symlink_metadata(path.join("cn.info")).is_ok(),
                    omitted_sidecars,
                },
            })
        })()
        .with_context(|| format!("merge input {index}: {}", path.display()))?;
        prepared.push(input);
    }
    let mut cn = std::collections::BTreeMap::new();
    let mut origins = HashMap::new();
    let mut cn_present = false;
    for input in &prepared {
        let info = copy_numbers::read_with_contigs(&input.canonical, &contigs)?;
        cn_present |= info.present;
        for (name, value) in info.explicit {
            if let Some(previous) = cn.insert(name.clone(), value) {
                ensure!(previous == value, "cn.info conflict for contig {name:?}: {} declares {previous}, {} declares {value}", origins[&name], input.canonical.display());
            } else { origins.insert(name, input.canonical.display().to_string()); }
        }
    }
    let kind = kind.unwrap();
    let mut writer = Writer::create(&target, kind, contigs, options.chunk_size)?;
    if cn_present { writer.set_copy_numbers(&cn)?; }
    // Declared after Writer so handles close before its Drop removes staging.
    let mut manifest = if options.provenance {
        Some(BufWriter::new(writer.merge_sidecar(false)?))
    } else {
        None
    };
    let mut mapping = if options.provenance && kind == Kind::Concat {
        Some(BufWriter::new(writer.merge_sidecar(true)?))
    } else {
        None
    };
    let mut output_id = 0u64;
    let mut sources = vec![];
    for mut input in prepared {
        (|| -> Result<()> {
            let mut reader = StreamingReader::open(
                &input.canonical,
                0,
                ReadOptions {
                    batch_rows: options.batch_rows,
                    boundary: if kind == Kind::Concat {
                        ReadBoundary::CompleteReads
                    } else {
                        ReadBoundary::Rows
                    },
                    concat_filter: None,
                },
            )?;
            if mapping.is_some() {
                reader.capture_origins();
            }
            while let Some(batch) = reader.next_columns()? {
                let rows = match batch {
                    ColumnBatch::Pairs(mut c) => {
                        remap(&mut c.chrom1, &input.map)?;
                        remap(&mut c.chrom2, &input.map)?;
                        writer.write_pairs_columns(c.as_view())?;
                        c.pos1.len()
                    }
                    ColumnBatch::Concat(mut c) => {
                        remap(&mut c.chrom, &input.map)?;
                        for (i, w) in c.read_offsets.windows(2).enumerate() {
                            increment(&mut output_id, 1)?;
                            if let Some(file) = &mut mapping {
                                let origin =
                                    reader.origins.get(i).context("missing read origin")?;
                                ensure!(
                                    origin.logical_id == c.read_idx[w[0] as usize],
                                    "read origin mismatch"
                                );
                                let value = obj([
                                    ("source_index", Value::from(input.source.index as u64)),
                                    (
                                        "output_read_id",
                                        Value::from(output_id.to_string().as_str()),
                                    ),
                                    (
                                        "logical_read_id",
                                        Value::from(origin.logical_id.to_string().as_str()),
                                    ),
                                    (
                                        "raw_read_id",
                                        Value::from(origin.raw_id.to_string().as_str()),
                                    ),
                                    (
                                        "shards",
                                        Value::List(
                                            origin
                                                .shards
                                                .iter()
                                                .map(|p| {
                                                    Value::from(
                                                        p.strip_prefix(&input.canonical)
                                                            .unwrap_or(p)
                                                            .to_string_lossy()
                                                            .as_ref(),
                                                    )
                                                })
                                                .collect(),
                                        ),
                                    ),
                                ]);
                                writeln!(file, "{}", value.json())?;
                            }
                            c.read_idx[w[0] as usize..w[1] as usize].fill(output_id);
                            increment(&mut input.source.output_reads, 1)?;
                        }
                        writer.write_concat_columns(c.as_view())?;
                        c.read_idx.len()
                    }
                };
                increment(&mut input.source.input_records, rows as u64)?;
                increment(&mut input.source.output_records, rows as u64)?;
            }
            if let Some(file) = &mut manifest {
                writeln!(file, "{}", input.source.to_value().json())?;
            }
            Ok(())
        })()
        .inspect_err(|_| writer.failed = true)
        .with_context(|| {
            format!(
                "merge input {}: {}",
                input.source.index,
                input.source.path.display()
            )
        })?;
        sources.push(input.source);
    }
    if let Some(mut file) = mapping {
        file.flush().inspect_err(|_| writer.failed = true)?;
    }
    if let Some(mut file) = manifest {
        file.flush().inspect_err(|_| writer.failed = true)?;
    }
    let counts = writer.finish()?;
    Ok(MergeResult {
        output: target,
        kind,
        counts,
        sources,
        provenance: options.provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_failure_poison_and_cleanup() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&root)?;
        let output = root.join(format!("merge-sidecar-failure-{}", std::process::id()));
        let staging;
        {
            let mut writer = Writer::create(
                &output,
                Kind::Pairs,
                vec![Contig {
                    name: "a".into(),
                    length: 10,
                }],
                1,
            )?;
            staging = writer.staging.clone();
            fs::create_dir(staging.join("_merge_sources.jsonl"))?;
            assert!(writer.merge_sidecar(false).is_err());
            assert!(writer.failed);
            assert!(writer.finish().is_err());
        }
        assert!(!output.exists());
        assert!(!staging.exists());
        Ok(())
    }
    #[test]
    fn overflow_is_checked() {
        let mut id = u64::MAX - 1;
        increment(&mut id, 1).unwrap();
        assert_eq!(id, u64::MAX);
        assert!(increment(&mut id, 1).is_err());
    }
}
