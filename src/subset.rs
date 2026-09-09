//! q0-only, order-preserving streaming subset. See docs/subset.md.
use crate::metadata::{obj, Value};
use crate::*;
use std::io::{BufWriter, Write};

pub const MAX_READ_IDS: usize = 100_000;
pub const MAX_ID_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug)]
pub enum ReadIds {
    Pairs(Vec<String>),
    Concat(Vec<u64>),
}
#[derive(Clone, Debug)]
pub struct SubsetOptions {
    pub min_mapq: Option<u8>,
    pub chroms: Option<Vec<String>>,
    pub regions: Option<Vec<Region>>,
    pub read_ids: Option<ReadIds>,
    /// None means Either for pairs; explicit policies are rejected for concat.
    pub pairs_mode: Option<PairsMode>,
    /// None means MatchingAlignments for concat; explicit modes reject pairs.
    pub mode: Option<ConcatFilter>,
    pub batch_rows: usize,
    pub chunk_size: usize,
    pub provenance: bool,
}
impl Default for SubsetOptions {
    fn default() -> Self {
        Self {
            min_mapq: None,
            chroms: None,
            regions: None,
            read_ids: None,
            pairs_mode: None,
            mode: None,
            batch_rows: 65_536,
            chunk_size: 1_000_000,
            provenance: true,
        }
    }
}
#[derive(Clone, Debug)]
pub struct SubsetResult {
    pub output: PathBuf,
    pub kind: Kind,
    pub counts: Counts,
    pub scanned_records: u64,
    pub full_scan: bool,
    pub index_used: bool,
    pub fallback_reason: String,
    pub provenance: Option<PathBuf>,
    pub omitted_sidecars: Vec<String>,
}
impl SubsetResult {
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
                    ("q0_records", self.counts.q0_records.into()),
                    ("q1_records", self.counts.q1_records.into()),
                    ("q0_concats", self.counts.q0_concats.into()),
                    ("q1_concats", self.counts.q1_concats.into()),
                ]),
            ),
            ("scanned_records", self.scanned_records.into()),
            ("full_scan", Value::Bool(self.full_scan)),
            ("index_used", Value::Bool(self.index_used)),
            (
                "fallback_reason",
                Value::from(self.fallback_reason.as_str()),
            ),
            (
                "provenance",
                self.provenance
                    .as_ref()
                    .map(|p| Value::from(p.to_string_lossy().as_ref()))
                    .unwrap_or(Value::Null),
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
    pub fn to_json(&self) -> String {
        self.to_value().json()
    }
}
fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => bail!("output or staging path already exists: {}", path.display()),
    }
}

/// Inputs must remain unchanged throughout the operation. Always scans q0;
/// batch_rows is a target, not a bound on row-group or single-read memory.
pub fn subset(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: SubsetOptions,
) -> Result<SubsetResult> {
    let input = input.as_ref();
    subset_inner(input, output.as_ref(), options)
        .with_context(|| format!("subset source {}", input.display()))
}
fn sidecar(writer: &mut Writer, ids: bool) -> Result<File> {
    let name = if ids {
        "_subset_read_ids.jsonl"
    } else {
        "_subset.json"
    };
    let result = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(writer.staging.join(name));
    if result.is_err() {
        writer.failed = true;
    }
    Ok(result?)
}
fn subset_inner(input: &Path, output: &Path, o: SubsetOptions) -> Result<SubsetResult> {
    ensure!(
        o.batch_rows > 0 && o.batch_rows <= u32::MAX as usize,
        "batch_rows must be in 1..=4294967295"
    );
    ensure!(o.chunk_size > 0, "chunk_size must be positive");
    let canonical = fs::canonicalize(input)?;
    let meta = Metadata::open(&canonical)?;
    let kind = meta.supported_kind()?;
    if let Some(scope) = meta.fields.get("read_idx_scope") {
        ensure!(
            matches!(scope.string(), Some("global" | "shard")),
            "unsupported read_idx_scope"
        );
    }
    if let Some(value) = meta.fields.get("is_with_mapq") {
        ensure!(
            value == &Value::Bool(true),
            "unsupported is_with_mapq semantics"
        );
    }
    if kind == Kind::Concat {
        for (key, expected) in [
            ("record_type", "porec_alignment"),
            ("partition_key", "read_idx"),
        ] {
            if let Some(value) = meta.fields.get(key) {
                ensure!(
                    value.string() == Some(expected),
                    "unsupported {key} semantics"
                );
            }
        }
    }
    ensure!(
        kind != Kind::Pairs || o.mode.is_none(),
        "pairs does not support concat mode"
    );
    ensure!(
        kind != Kind::Concat || o.pairs_mode.is_none(),
        "concat does not support pairs_mode"
    );
    let mut reader = StreamingReader::open(
        &canonical,
        0,
        ReadOptions {
            batch_rows: o.batch_rows,
            boundary: if kind == Kind::Concat {
                ReadBoundary::CompleteReads
            } else {
                ReadBoundary::Rows
            },
            concat_filter: None,
        },
    )?;
    let contigs = reader.contigs().to_vec();
    // Same validated/merged region intervals and exact overlap as QueryReader.
    let predicate = query::Predicate::new(
        &contigs,
        &QueryOptions {
            regions: o.regions.clone().unwrap_or_default(),
            ..QueryOptions::default()
        },
    )?;
    let chroms = o
        .chroms
        .as_ref()
        .map(|names| {
            names
                .iter()
                .map(|name| {
                    contigs
                        .iter()
                        .position(|c| &c.name == name)
                        .map(|i| i as u32)
                        .with_context(|| format!("unknown chroms contig: {name}"))
                })
                .collect::<Result<HashSet<_>>>()
        })
        .transpose()?;
    let mut pair_ids = None;
    let mut concat_ids = None;
    match &o.read_ids {
        Some(ReadIds::Pairs(ids)) => {
            ensure!(
                kind == Kind::Pairs,
                "concat read_ids must be UInt64 logical IDs"
            );
            ensure!(ids.len() <= MAX_READ_IDS, "read_ids exceeds 100000 entries");
            ensure!(
                ids.iter()
                    .try_fold(0usize, |n, s| n.checked_add(s.len()))
                    .is_some_and(|n| n <= MAX_ID_BYTES),
                "read_ids exceeds 4 MiB UTF-8 bytes"
            );
            pair_ids = Some(ids.iter().map(String::as_bytes).collect::<HashSet<_>>());
        }
        Some(ReadIds::Concat(ids)) => {
            ensure!(kind == Kind::Concat, "pairs read_ids must be strings");
            ensure!(ids.len() <= MAX_READ_IDS, "read_ids exceeds 100000 entries");
            concat_ids = Some(ids.iter().copied().collect::<HashSet<_>>());
        }
        None => (),
    }
    absent(output)?;
    let leaf = output
        .file_name()
        .context("output must name a new dataset directory")?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let target = fs::canonicalize(parent)
        .context("output parent must exist")?
        .join(leaf);
    absent(&target)?;
    let mut staging = target.as_os_str().to_os_string();
    staging.push(".partial");
    let staging = PathBuf::from(staging);
    absent(&staging)?;
    for source in [
        canonical.clone(),
        fs::canonicalize(canonical.join("q0"))?,
        fs::canonicalize(canonical.join("q1"))?,
    ] {
        for dest in [&target, &staging] {
            ensure!(
                !dest.starts_with(&source) && !source.starts_with(dest),
                "output/staging overlaps input"
            );
        }
    }
    let mut omitted_sidecars = Vec::new();
    for entry in fs::read_dir(&canonical)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !matches!(
            name.as_str(),
            "q0" | "q1" | "_metadata" | "_metadata_counts" | "_contigsizes" | "_readme"
        ) {
            omitted_sidecars.push(name);
        }
    }
    omitted_sidecars.sort();
    let spatial = |chrom, start, end| {
        chroms.as_ref().is_none_or(|cs| cs.contains(&chrom))
            && (o.regions.is_none() || predicate.overlap(chrom, start, end))
    };
    let mut writer = Writer::create(&target, kind, contigs, o.chunk_size)?;
    let mut result = SubsetResult { output: target.clone(), kind, counts: Counts::default(),
        scanned_records: 0, full_scan: false, index_used: false,
        fallback_reason: "subset uses sequential q0 scan to preserve logical IDs, order and complete read groups; region index has no read locator".into(),
        provenance: o.provenance.then(|| target.join("_subset.json")), omitted_sidecars };
    while let Some(batch) = reader.next_columns()? {
        match batch {
            ColumnBatch::Pairs(c) => {
                result.scanned_records += c.pos1.len() as u64;
                let mut selected = PairColumns::default();
                for i in 0..c.pos1.len() {
                    writer
                        .pair_fields(
                            c.chrom1[i],
                            c.pos1[i],
                            c.chrom2[i],
                            c.pos2[i],
                            c.strand1[i],
                            c.strand2[i],
                        )
                        .with_context(|| {
                            format!(
                                "pairs fields at scanned record {}",
                                result.scanned_records - c.pos1.len() as u64 + i as u64
                            )
                        })?;
                    let a = spatial(c.chrom1[i], c.pos1[i] - 1, c.pos1[i]);
                    let b = spatial(c.chrom2[i], c.pos2[i] - 1, c.pos2[i]);
                    let id = &c.read_id_bytes
                        [c.read_id_offsets[i] as usize..c.read_id_offsets[i + 1] as usize];
                    if c.mapq[i] >= o.min_mapq.unwrap_or(0)
                        && pair_ids.as_ref().is_none_or(|ids| ids.contains(id))
                        && (if o.pairs_mode == Some(PairsMode::Both) {
                            a && b
                        } else {
                            a || b
                        })
                    {
                        selected.append(c.as_view(), i, i + 1);
                    }
                }
                writer.write_pairs_columns(selected.as_view())?;
            }
            ColumnBatch::Concat(c) => {
                result.scanned_records += c.read_idx.len() as u64;
                let mut selected = ConcatColumns::default();
                for w in c.read_offsets.windows(2) {
                    let (a, b) = (w[0] as usize, w[1] as usize);
                    for i in a..b {
                        ensure!(
                            c.read_length[i] == c.read_length[a],
                            "read_length disagrees within read {}",
                            c.read_idx[a]
                        );
                        writer
                            .alignment_fields(
                                (c.read_length[i], c.read_start[i], c.read_end[i]),
                                (c.chrom[i], c.start[i], c.end[i]),
                                c.strand[i],
                                c.identity[i],
                            )
                            .with_context(|| {
                                format!("alignment fields for logical read {}", c.read_idx[i])
                            })?;
                    }
                    let hit = |i: usize| {
                        c.mapping_quality[i] >= o.min_mapq.unwrap_or(0)
                            && concat_ids
                                .as_ref()
                                .is_none_or(|ids| ids.contains(&c.read_idx[i]))
                            && spatial(c.chrom[i], c.start[i], c.end[i])
                    };
                    if o.mode == Some(ConcatFilter::CompleteReads) {
                        if (a..b).any(hit) {
                            selected.append(c.as_view(), a, b);
                        }
                    } else {
                        for i in a..b {
                            if hit(i) {
                                selected.append(c.as_view(), i, i + 1);
                            }
                        }
                    }
                }
                writer.write_concat_columns(selected.as_view())?;
            }
        }
    }
    writer.flush_columns()?;
    result.counts = writer.counts;
    result.full_scan = true;
    if o.provenance {
        // IDs are line-oriented and bounded, never embedded in the manifest JSON.
        if let Some(ids) = &o.read_ids {
            let mut file = BufWriter::new(sidecar(&mut writer, true)?);
            match ids {
                ReadIds::Pairs(ids) => {
                    for id in ids {
                        writeln!(file, "{}", Value::from(id.as_str()).json())?;
                    }
                }
                ReadIds::Concat(ids) => {
                    for id in ids {
                        writeln!(file, "{id}")?;
                    }
                }
            }
            file.flush()?;
        }
        let filters = obj([
            (
                "min_mapq",
                o.min_mapq
                    .map(|q| Value::from(q as u64))
                    .unwrap_or(Value::Null),
            ),
            (
                "chroms",
                o.chroms
                    .as_ref()
                    .map(|cs| Value::List(cs.iter().map(|s| Value::from(s.as_str())).collect()))
                    .unwrap_or(Value::Null),
            ),
            (
                "regions",
                o.regions
                    .as_ref()
                    .map(|rs| {
                        Value::List(
                            rs.iter()
                                .map(|r| {
                                    Value::List(vec![
                                        Value::from(r.contig.as_str()),
                                        r.start.into(),
                                        r.end.into(),
                                    ])
                                })
                                .collect(),
                        )
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "read_ids",
                if o.read_ids.is_some() {
                    Value::from("_subset_read_ids.jsonl")
                } else {
                    Value::Null
                },
            ),
            (
                "pairs_mode",
                if kind == Kind::Pairs {
                    Value::from(if o.pairs_mode == Some(PairsMode::Both) {
                        "both"
                    } else {
                        "either"
                    })
                } else {
                    Value::Null
                },
            ),
            (
                "mode",
                if kind == Kind::Concat {
                    Value::from(if o.mode == Some(ConcatFilter::CompleteReads) {
                        "complete_reads"
                    } else {
                        "matching_alignments"
                    })
                } else {
                    Value::Null
                },
            ),
        ]);
        let manifest = obj([
            ("schema_version", 1u64.into()),
            (
                "source_path_label",
                Value::from(input.to_string_lossy().as_ref()),
            ),
            (
                "format_version",
                Value::from(meta.get_str("format-version").unwrap()),
            ),
            (
                "input_read_id_scope",
                meta.get_str("read_idx_scope")
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            (
                "id_relationship",
                Value::from(if kind == Kind::Pairs {
                    "unchanged string IDs"
                } else {
                    "output global IDs equal input public logical IDs; shard-local disk IDs use Reader deterministic q0 mapping"
                }),
            ),
            ("filters", filters),
            ("result", result.to_value()),
        ]);
        let mut file = BufWriter::new(sidecar(&mut writer, false)?);
        writeln!(file, "{}", manifest.json())?;
        file.flush()?;
    }
    result.counts = writer.finish()?;
    Ok(result)
}

// Additive ABI options use the existing non-executing, size/depth-limited parser.
pub(crate) fn parse_options(text: &str, kind: Kind) -> Result<SubsetOptions> {
    let m = Metadata::parse(text)?;
    let mut o = SubsetOptions::default();
    for (key, v) in &m.fields {
        if *v == Value::Null
            && matches!(
                key.as_str(),
                "min_mapq" | "chroms" | "regions" | "read_ids" | "pairs_mode" | "mode"
            )
        {
            continue;
        }
        let number = || {
            v.u64()
                .with_context(|| format!("{key} must be an unsigned integer"))
        };
        let list = || match v {
            Value::List(xs) => Ok(xs),
            _ => bail!("{key} must be a list"),
        };
        match key.as_str() {
            "min_mapq" => {
                o.min_mapq = Some(u8::try_from(number()?).context("min_mapq must be in 0..=255")?)
            }
            "batch_rows" => o.batch_rows = usize::try_from(number()?)?,
            "chunk_size" => o.chunk_size = usize::try_from(number()?)?,
            "provenance" => {
                let Value::Bool(b) = v else {
                    bail!("provenance must be bool")
                };
                o.provenance = *b;
            }
            "pairs_mode" => {
                o.pairs_mode = Some(match v.string() {
                    Some("either") => PairsMode::Either,
                    Some("both") => PairsMode::Both,
                    _ => bail!("pairs_mode must be either or both"),
                })
            }
            "mode" => {
                o.mode = Some(match v.string() {
                    Some("matching_alignments") => ConcatFilter::MatchingAlignments,
                    Some("complete_reads") => ConcatFilter::CompleteReads,
                    _ => bail!("mode must be matching_alignments or complete_reads"),
                })
            }
            "chroms" => {
                o.chroms = Some(
                    list()?
                        .iter()
                        .map(|v| {
                            v.string()
                                .map(str::to_owned)
                                .context("chroms must contain strings")
                        })
                        .collect::<Result<_>>()?,
                )
            }
            "regions" => {
                o.regions = Some(
                    list()?
                        .iter()
                        .map(|v| {
                            let Value::List(r) = v else {
                                bail!("region must be [contig, start, end]")
                            };
                            ensure!(r.len() == 3, "region must be [contig, start, end]");
                            Ok(Region {
                                contig: r[0]
                                    .string()
                                    .context("region contig must be string")?
                                    .into(),
                                start: r[1].u64().context("region start must be UInt64")?,
                                end: r[2].u64().context("region end must be UInt64")?,
                            })
                        })
                        .collect::<Result<_>>()?,
                )
            }
            "read_ids" => {
                let xs = list()?;
                ensure!(xs.len() <= MAX_READ_IDS, "read_ids exceeds 100000 entries");
                o.read_ids = Some(if kind == Kind::Pairs {
                    ReadIds::Pairs(
                        xs.iter()
                            .map(|v| {
                                v.string()
                                    .map(str::to_owned)
                                    .context("pairs read_ids must contain strings")
                            })
                            .collect::<Result<_>>()?,
                    )
                } else {
                    ReadIds::Concat(
                        xs.iter()
                            .map(|v| {
                                v.u64()
                                    .context("concat read_ids must contain UInt64 logical IDs")
                            })
                            .collect::<Result<_>>()?,
                    )
                });
            }
            _ => bail!("unknown subset option: {key}"),
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provenance_failure_prevents_publication_and_cleans_staging() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&root)?;
        let output = root.join(format!("subset-sidecar-failure-{}", std::process::id()));
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
            fs::create_dir(staging.join("_subset.json"))?;
            assert!(sidecar(&mut writer, false).is_err());
            assert!(writer.finish().is_err());
        }
        assert!(!output.exists());
        assert!(!staging.exists());
        Ok(())
    }
    #[test]
    fn abi_options_are_typed_and_non_executing() -> Result<()> {
        let o = parse_options(
            r#"{"provenance":false,"min_mapq":null,"regions":[],"read_ids":[18446744073709551615]}"#,
            Kind::Concat,
        )?;
        assert!(!o.provenance);
        assert!(o.min_mapq.is_none());
        assert!(o.regions.unwrap().is_empty());
        for bad in [
            r#"{"min_mapq":256}"#,
            r#"{"read_ids":[true]}"#,
            r#"{"regions":[["a",-1,2]]}"#,
            r#"{"unknown":1}"#,
            "__import__('os')",
        ] {
            assert!(parse_options(bad, Kind::Concat).is_err(), "{bad}");
        }
        Ok(())
    }
}
