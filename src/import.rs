//! Native BAM/PAF import. Parsing, external sorting, expansion and PQS output
//! stay inside Rust; the language bindings make one call per conversion.
use crate::metadata::{obj, Value};
use crate::*;
use std::io::Write;
mod bam;
mod cigar;
mod paf;
mod sort;

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub chunk_size: usize,
    pub batch_rows: usize,
    pub min_mapq: u8,
    /// None selects 2 for pairs or 1 for concat.
    pub min_order: Option<usize>,
    pub max_order: Option<usize>,
    /// Decoder/compressor and Parquet encoding worker limit (not a total CPU cap).
    pub threads: usize,
    pub contigsizes: Option<PathBuf>,
    pub include_secondary: bool,
    pub tmpdir: Option<PathBuf>,
    pub five_prime: bool,
}
impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            chunk_size: 1_000_000,
            batch_rows: 65_536,
            min_mapq: 0,
            min_order: None,
            max_order: None,
            threads: 1,
            contigsizes: None,
            include_secondary: false,
            tmpdir: None,
            five_prime: false,
        }
    }
}

/// Invalid options or biological records; bindings expose these as ValueError.
#[derive(Debug)]
pub struct InvalidInput(String);
impl std::fmt::Display for InvalidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for InvalidInput {}
fn invalid<T>(result: Result<T>) -> Result<T> {
    result.map_err(|e| InvalidInput(format!("{e:#}")).into())
}

#[derive(Debug)]
struct Record {
    rg: String,
    name: String,
    ordinal: u64,
    mate: u8,
    row: Alignment,
}

#[derive(Default)]
struct References {
    contigs: Vec<Contig>,
    ids: HashMap<String, u32>,
}
impl References {
    fn add(&mut self, name: &str, length: u64) -> Result<u32> {
        ensure!(
            !name.is_empty()
                && name != "*"
                && !name.contains('\0')
                && !name.chars().any(char::is_whitespace),
            "invalid contig name: {name:?}"
        );
        ensure!(length > 0, "contig length must be positive");
        if let Some(&id) = self.ids.get(name) {
            ensure!(
                self.contigs[id as usize].length == length,
                "conflicting lengths for contig {name}"
            );
            return Ok(id);
        }
        let id = u32::try_from(self.contigs.len())?;
        self.contigs.push(Contig {
            name: name.into(),
            length,
        });
        self.ids.insert(name.into(), id);
        Ok(id)
    }
}

#[derive(Default)]
struct Statistics {
    input_records: u64,
    skipped_unmapped: u64,
    skipped_secondary: u64,
    skipped_duplicate_or_qcfail: u64,
    skipped_mapq: u64,
    selected_read_groups: u64,
    skipped_order_groups: u64,
}
impl Statistics {
    fn value(&self) -> Value {
        obj([
            ("input_records", self.input_records),
            ("skipped_unmapped", self.skipped_unmapped),
            ("skipped_secondary", self.skipped_secondary),
            (
                "skipped_duplicate_or_qcfail",
                self.skipped_duplicate_or_qcfail,
            ),
            ("skipped_mapq", self.skipped_mapq),
            ("selected_read_groups", self.selected_read_groups),
            ("skipped_order_groups", self.skipped_order_groups),
        ]
        .map(|(k, v)| (k, Value::from(v))))
    }
}

#[derive(Clone, Debug)]
pub struct ImportResult {
    pub counts: Counts,
    report: Value,
}
impl ImportResult {
    pub fn to_json(&self) -> String {
        self.report.json()
    }
}

/// Convert BAM (Hi-C or long-read) or PAF directly to pairs/concat PQS.
/// Scratch files reside in tmpdir or the output parent and are removed on error.
pub fn import_alignments(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mode: &str,
    options: ImportOptions,
) -> Result<ImportResult> {
    let source = std::path::absolute(input)?;
    let target = std::path::absolute(output)?;
    let pairs = matches!(mode, "bam2pairs" | "paf2pairs");
    let paf = matches!(mode, "paf2pairs" | "paf2concat");
    let mut staging = target.as_os_str().to_os_string();
    staging.push(".partial");
    invalid((|| {
        ensure!(
            matches!(
                mode,
                "bam2pairs" | "bam2concat" | "paf2pairs" | "paf2concat"
            ),
            "unknown import mode: {mode}"
        );
        ensure!(
            source.is_file(),
            "input must be an existing BAM/PAF file: {}",
            source.display()
        );
        for path in [&target, &PathBuf::from(staging)] {
            match fs::symlink_metadata(path) {
                Ok(_) => bail!("output or staging path already exists: {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        ensure!(
            target.parent().is_some_and(Path::is_dir),
            "output parent directory does not exist"
        );
        ensure!(
            options.threads > 0 && options.chunk_size > 0 && options.batch_rows > 0,
            "threads, chunk_size and batch_rows must be positive"
        );
        ensure!(
            options.batch_rows <= u32::MAX as usize,
            "batch_rows must fit UInt32"
        );
        let minimum = if pairs { 2 } else { 1 };
        let min_order = options.min_order.unwrap_or(minimum);
        ensure!(min_order >= minimum, "min_order must be at least {minimum}");
        ensure!(
            options.max_order.is_none_or(|m| m > min_order),
            "max_order must exceed min_order (exclusive upper bound)"
        );
        ensure!(
            paf || options.contigsizes.is_none(),
            "contigsizes is supported only for PAF imports"
        );
        ensure!(
            pairs || !options.five_prime,
            "pair_position is supported only for pairs imports"
        );
        Ok(())
    })())?;
    let scratch = sort::Scratch::new(
        options
            .tmpdir
            .as_deref()
            .unwrap_or(target.parent().unwrap()),
    )?;
    let mut sorter = sort::Sorter::new(&scratch.0, 32 * 1024 * 1024);
    progress::emit("Reading and grouping alignments", 0, 0);
    let mut refs = References::default();
    let mut stats = Statistics::default();
    if paf {
        paf::load(&source, &options, &mut refs, &mut stats, &mut sorter)?;
    } else {
        bam::load(
            &source,
            &options,
            !pairs,
            &mut refs,
            &mut stats,
            &mut sorter,
        )?;
    }
    invalid((|| {
        ensure!(
            !refs.contigs.is_empty(),
            "no reference contigs found; empty PAF input requires --contigsizes"
        );
        Ok(())
    })())?;
    progress::emit("Preparing alignment merge", stats.input_records, 0);
    let mut merged = sorter.finish()?;
    let mut writer = Writer::create(
        &target,
        if pairs { Kind::Pairs } else { Kind::Concat },
        refs.contigs,
        options.chunk_size,
    )?;
    if options.threads > 1 {
        writer.parallel_encoding(options.threads)?;
    }
    let mut names = if pairs {
        None
    } else {
        Some(crate::io::common_writer(
            writer.staging.join("_import_read_names.jsonl"),
            options.threads,
        )?)
    };
    let mut pending = merged.next()?;
    let mut columns = PairColumns::default();
    let mut read_idx = 0;
    let mut counts = Counts::default();
    progress::emit("Expanding and writing PQS", 0, 0);
    while let Some(first) = pending.take() {
        let mut group = vec![first];
        loop {
            match merged.next()? {
                Some(record) if record.rg == group[0].rg && record.name == group[0].name => {
                    group.push(record)
                }
                other => {
                    pending = other;
                    break;
                }
            }
        }
        stats.selected_read_groups += 1;
        if stats.selected_read_groups.is_multiple_of(1024) {
            progress::emit("Expanding and writing PQS", stats.selected_read_groups, 0);
        }
        invalid((|| {
            ensure!(
                !(group[0].mate == 0 && group.last().unwrap().mate != 0),
                "read {:?}: mixed paired and unpaired records in RG {:?}",
                group[0].name,
                group[0].rg
            );
            if paf || !pairs {
                for rows in group.chunk_by(|a, b| a.mate == b.mate) {
                    ensure!(
                        rows.iter()
                            .all(|r| r.row.read_length == rows[0].row.read_length),
                        "read {:?}: inconsistent query lengths across alignments",
                        rows[0].name
                    );
                }
            }
            Ok(())
        })())?;
        let keep = |n: usize| {
            n >= options.min_order.unwrap_or(if pairs { 2 } else { 1 })
                && options.max_order.is_none_or(|m| n < m)
        };
        if pairs {
            if !keep(group.len()) {
                stats.skipped_order_groups += 1;
                continue;
            }
            expand(&group, &mut columns, &mut writer, &options, &mut counts)?;
        } else {
            for rows in group.chunk_by(|a, b| a.mate == b.mate) {
                if !keep(rows.len()) {
                    stats.skipped_order_groups += 1;
                    continue;
                }
                read_idx += 1;
                let alignments: Vec<_> = rows
                    .iter()
                    .map(|r| {
                        let mut row = r.row.clone();
                        row.read_idx = read_idx;
                        row
                    })
                    .collect();
                writer.write_read(&alignments)?;
                counts.q0_records += rows.len() as u64;
                let quality = rows.iter().filter(|r| r.row.mapping_quality > 0).count() as u64;
                counts.q1_records += quality;
                counts.q0_concats += 1;
                counts.q1_concats += u64::from(quality > 0);
                let r = &rows[0];
                let name = obj([
                    ("read_idx", Value::from(read_idx)),
                    ("read_name", Value::from(r.name.as_str())),
                    ("read_group", Value::from(r.rg.as_str())),
                    (
                        "mate",
                        if r.mate == 0 {
                            Value::Null
                        } else {
                            Value::from(r.mate as u64)
                        },
                    ),
                ]);
                writeln!(names.as_mut().unwrap(), "{}", name.json())?;
            }
        }
    }
    flush_pairs(&mut columns, &mut writer)?;
    if let Some(names) = names {
        names.finish()?;
    }
    let report = obj([
        ("source", Value::from(source.to_string_lossy().as_ref())),
        ("mode", Value::from(mode)),
        ("output", Value::from(target.to_string_lossy().as_ref())),
        (
            "format",
            Value::from(if pairs { "pairs" } else { "concat" }),
        ),
        (
            "options",
            obj([
                ("min_mapq", Value::from(options.min_mapq as u64)),
                (
                    "min_order",
                    Value::from(options.min_order.unwrap_or(if pairs { 2 } else { 1 }) as u64),
                ),
                (
                    "max_order",
                    options
                        .max_order
                        .map(|n| Value::from(n as u64))
                        .unwrap_or(Value::Null),
                ),
                ("include_secondary", Value::Bool(options.include_secondary)),
                (
                    "pair_position",
                    if pairs {
                        Value::from(if options.five_prime {
                            "five-prime"
                        } else {
                            "leftmost"
                        })
                    } else {
                        Value::Null
                    },
                ),
                (
                    "contigsizes",
                    options
                        .contigsizes
                        .as_ref()
                        .map(|p| Value::from(p.to_string_lossy().as_ref()))
                        .unwrap_or(Value::Null),
                ),
            ]),
        ),
        ("counts", {
            let mut c = vec![
                ("q0_records", Value::from(counts.q0_records)),
                ("q1_records", Value::from(counts.q1_records)),
            ];
            if !pairs {
                c.extend([
                    ("q0_concats", Value::from(counts.q0_concats)),
                    ("q1_concats", Value::from(counts.q1_concats)),
                ]);
            }
            obj(c)
        }),
        ("statistics", stats.value()),
        (
            "read_names",
            if pairs {
                Value::Null
            } else {
                Value::from(
                    target
                        .join("_import_read_names.jsonl")
                        .to_string_lossy()
                        .as_ref(),
                )
            },
        ),
    ]);
    let mut manifest =
        crate::io::common_writer(writer.staging.join("_import.json"), options.threads)?;
    writeln!(manifest, "{}", report.json())?;
    manifest.finish()?;
    writer.finish()?;
    Ok(ImportResult { counts, report })
}

fn flush_pairs(p: &mut PairColumns, writer: &mut Writer) -> Result<()> {
    if p.pos1.is_empty() {
        return Ok(());
    }
    writer.write_pairs_columns(p.as_view())?;
    p.read_id_bytes.clear();
    p.read_id_offsets.clear();
    p.read_id_offsets.push(0);
    p.chrom1.clear();
    p.chrom2.clear();
    p.pos1.clear();
    p.pos2.clear();
    p.strand1.clear();
    p.strand2.clear();
    p.mapq.clear();
    Ok(())
}

fn expand(
    group: &[Record],
    p: &mut PairColumns,
    writer: &mut Writer,
    options: &ImportOptions,
    counts: &mut Counts,
) -> Result<()> {
    let ends: Vec<_> = group
        .iter()
        .map(|r| {
            let a = &r.row;
            let position = if options.five_prime && a.strand == b'-' {
                a.end
            } else {
                a.start + 1
            };
            (a.chrom, position, a.strand, a.mapping_quality)
        })
        .collect();
    let indices: Vec<_> = (0..ends.len()).map(|i| i.to_string()).collect();
    for left in 0..ends.len() - 1 {
        let prefix = format!("{}:{}:", group[0].name, indices[left]);
        for right in left + 1..ends.len() {
            let (mut a, mut b) = (ends[left], ends[right]);
            if (a.0, a.1) > (b.0, b.1) {
                std::mem::swap(&mut a, &mut b);
            }
            let quality = a.3.min(b.3);
            p.read_id_bytes.extend_from_slice(prefix.as_bytes());
            p.read_id_bytes.extend_from_slice(indices[right].as_bytes());
            p.read_id_offsets.push(p.read_id_bytes.len() as u64);
            p.chrom1.push(a.0);
            p.pos1.push(a.1);
            p.strand1.push(a.2);
            p.chrom2.push(b.0);
            p.pos2.push(b.1);
            p.strand2.push(b.2);
            p.mapq.push(quality);
            counts.q0_records += 1;
            counts.q1_records += u64::from(quality > 0);
            if p.pos1.len() >= options.batch_rows {
                flush_pairs(p, writer)?;
            }
        }
    }
    Ok(())
}
