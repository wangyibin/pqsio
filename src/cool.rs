//! Native pairs PQS / .pairs text to single-resolution Cooler v3.
use crate::metadata::{obj, Value};
use crate::progress;
use crate::{Contig, Kind, Reader};
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
mod compressed;
mod pqs;
mod sort;
mod sort_pool;
mod text;
mod write;

#[derive(Clone, Debug)]
pub struct CoolOptions {
    pub bin_size: u64,
    /// Accepted pair records per sorted run; one run stays in memory.
    pub chunk_size: usize,
    pub batch_rows: usize,
    pub min_mapq: u8,
    /// Workers per PQS decoding/binning, sorting or pixel compression stage.
    /// HDF5 calls remain serialized on the calling thread.
    pub threads: usize,
    pub contigsizes: Option<PathBuf>,
    pub tmpdir: Option<PathBuf>,
}
impl Default for CoolOptions {
    fn default() -> Self {
        Self {
            bin_size: 10_000,
            chunk_size: 1_000_000,
            batch_rows: 65_536,
            min_mapq: 0,
            threads: 1,
            contigsizes: None,
            tmpdir: None,
        }
    }
}
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

struct References {
    contigs: Vec<Contig>,
    ids: HashMap<String, usize>,
    offsets: Vec<i64>,
    bin_size: u64,
}
impl References {
    fn new(bin_size: u64) -> Self {
        Self {
            contigs: vec![],
            ids: HashMap::new(),
            offsets: vec![0],
            bin_size,
        }
    }
    fn add(&mut self, name: &str, length: u64) -> Result<()> {
        ensure!(
            !name.is_empty()
                && name.is_ascii()
                && !name.contains('\0')
                && !name.chars().any(char::is_whitespace)
                && !matches!(name, "!" | "*"),
            "Cooler chromosome names must be nonempty ASCII without whitespace or NUL: {name:?}"
        );
        ensure!(
            length > 0 && length <= i64::MAX as u64,
            "chromosome length must fit positive Int64"
        );
        if let Some(&id) = self.ids.get(name) {
            ensure!(
                self.contigs[id].length == length,
                "conflicting length for chromosome {name}"
            );
        } else {
            ensure!(
                self.contigs.len() < i32::MAX as usize,
                "too many chromosomes for Cooler"
            );
            let bins = i64::try_from(length.div_ceil(self.bin_size))?;
            let offset = self
                .offsets
                .last()
                .unwrap()
                .checked_add(bins)
                .context("bin count exceeds Int64")?;
            self.ids.insert(name.into(), self.contigs.len());
            self.contigs.push(Contig {
                name: name.into(),
                length,
            });
            self.offsets.push(offset);
        }
        Ok(())
    }
    fn bin(&self, chrom: usize, position: u64) -> Result<i64> {
        let reference = self
            .contigs
            .get(chrom)
            .context("chromosome ID out of range")?;
        ensure!(
            position > 0 && position <= reference.length,
            "1-based position {position} is outside chromosome {} (length {})",
            reference.name,
            reference.length
        );
        Ok(self.offsets[chrom] + ((position - 1) / self.bin_size) as i64)
    }
}
#[derive(Default)]
struct Statistics {
    input_records: u64,
    skipped_mapq: u64,
    skipped_unmapped: u64,
    contacts: u64,
}

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Guard {
    path: PathBuf,
    directory: bool,
}
impl Guard {
    fn scratch(parent: &Path) -> Result<Self> {
        loop {
            let path = parent.join(format!(
                ".pqsio-cool-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        directory: true,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).context("cannot create Cooler scratch directory"),
            }
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.directory {
            let _ = fs::remove_dir_all(&self.path);
        } else {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Debug)]
pub struct CoolResult {
    report: Value,
}
impl CoolResult {
    pub fn to_json(&self) -> String {
        self.report.json()
    }
}

/// Aggregate all selected pair records (including duplicates/diagonal contacts)
/// into sorted symmetric-upper Cooler pixels. Existing paths are never replaced.
pub fn pairs2cool(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: CoolOptions,
) -> Result<CoolResult> {
    let source = std::path::absolute(input)?;
    let target = std::path::absolute(output)?;
    let mut partial = target.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    invalid((|| {
        ensure!(
            options.bin_size > 0 && options.bin_size <= i64::MAX as u64,
            "bin_size must fit positive Int64"
        );
        ensure!(
            options.threads > 0
                && options.chunk_size > 0
                && options.batch_rows > 0
                && options.batch_rows <= u32::MAX as usize,
            "threads/chunk_size/batch_rows must be positive and batch_rows must fit UInt32"
        );
        ensure!(
            !source.to_string_lossy().contains("::") && !target.to_string_lossy().contains("::"),
            "pairs2cool requires local paths, not HDF5 group URIs"
        );
        ensure!(
            source.is_dir() || source.is_file(),
            "input must be an existing pairs PQS directory or .pairs file"
        );
        ensure!(
            target.parent().is_some_and(Path::is_dir),
            "output parent directory does not exist"
        );
        for path in [&target, &partial] {
            match fs::symlink_metadata(path) {
                Ok(_) => bail!("output or staging path already exists: {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        ensure!(
            !source.is_dir() || options.contigsizes.is_none(),
            "pairs PQS uses its own reference dictionary; omit contigsizes"
        );
        Ok(())
    })())?;
    let scratch = Guard::scratch(
        options
            .tmpdir
            .as_deref()
            .unwrap_or(target.parent().unwrap()),
    )?;
    let mut sorter = sort::Sorter::new(&scratch.path, options.chunk_size, options.threads);
    let mut refs = References::new(options.bin_size);
    let mut stats = Statistics::default();
    progress::emit("Reading, binning and sorting pairs", 0, 0);
    if source.is_dir() {
        // q0 contains every contact; q1 must never be added to it.
        let reader = Reader::open(&source, 0)?;
        invalid((|| {
            ensure!(
                reader.kind == Kind::Pairs,
                "pairs2cool requires pairs PQS; convert concat2pairs first"
            );
            for contig in &reader.contigs {
                refs.add(&contig.name, contig.length)?;
            }
            Ok(())
        })())?;
        pqs::load(reader.files, &refs, &options, &mut stats, &mut sorter)?;
    } else {
        text::load(&source, &options, &mut refs, &mut stats, &mut sorter)?;
    }
    invalid((|| {
        ensure!(
            !refs.contigs.is_empty(),
            "no chromosome sizes found; provide --contigsizes or #chromsize headers"
        );
        Ok(())
    })())?;
    progress::emit("Preparing sorted-run merge", stats.input_records, 0);
    let mut pixels = sorter.finish()?;
    // Reserve ownership before HDF5 opens/truncates the private staging file.
    drop(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?,
    );
    let staging = Guard {
        path: partial,
        directory: false,
    };
    progress::emit("Merging, compressing and writing Cooler", 0, 0);
    let nnz = write::cooler(&staging.path, &source, &refs, &mut pixels, &stats, &options)?;
    // Hard-link publication is atomic and fails if a concurrent creator won.
    // Both paths share the output directory/filesystem; Drop removes staging.
    progress::emit("Publishing Cooler", nnz, 0);
    fs::hard_link(&staging.path, &target)
        .context("cannot publish Cooler without replacing existing output")?;
    Ok(CoolResult {
        report: obj([
            ("mode", Value::from("pairs2cool")),
            ("format", Value::from("cool")),
            ("output", Value::from(target.to_string_lossy().as_ref())),
            ("bin_size", Value::from(options.bin_size)),
            ("nchroms", Value::from(refs.contigs.len() as u64)),
            ("nbins", Value::from(*refs.offsets.last().unwrap() as u64)),
            ("nnz", Value::from(nnz)),
            ("sum", Value::from(stats.contacts)),
            ("input_records", Value::from(stats.input_records)),
            ("skipped_mapq", Value::from(stats.skipped_mapq)),
            ("skipped_unmapped", Value::from(stats.skipped_unmapped)),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_run_stays_in_memory_including_exact_limit() -> Result<()> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&parent)?;
        for threads in [1, 4] {
            for size in [0, 10, 150_000] {
                let scratch = Guard::scratch(&parent)?;
                let mut sorter = sort::Sorter::new(&scratch.path, size.max(1), threads);
                let mut expected = std::collections::BTreeMap::new();
                for i in 0..size as i64 {
                    let (a, b) = (i % 97, i % 89);
                    sorter.push(a, b)?;
                    *expected.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
                let mut merged = sorter.finish()?;
                assert_eq!(
                    fs::read_dir(&scratch.path)?.count(),
                    0,
                    "single run created a spill file"
                );
                for ((bin1, bin2), count) in expected {
                    assert_eq!(merged.next()?, Some(sort::Pixel { bin1, bin2, count }));
                }
                assert_eq!(merged.next()?, None);
            }
        }
        Ok(())
    }

    #[test]
    fn parallel_spill_errors_are_joined() -> Result<()> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&parent)?;
        let scratch = Guard::scratch(&parent)?;
        let result = (|| -> Result<()> {
            let mut sorter = sort::Sorter::new(&scratch.path.join("missing"), 1, 4);
            for i in 0..100 {
                sorter.push(0, i)?;
            }
            sorter.finish()?;
            Ok(())
        })();
        assert!(result.is_err());
        let mut pool = sort_pool::Pool::new(4)?;
        pool.submit(
            vec![
                sort::Pixel {
                    bin1: 0,
                    bin2: 0,
                    count: i64::MAX,
                },
                sort::Pixel {
                    bin1: 0,
                    bin2: 0,
                    count: 1,
                },
            ],
            scratch.path.join("overflow"),
        )?;
        assert!(pool
            .finish()
            .unwrap_err()
            .to_string()
            .contains("exceeds Int64"));
        drop(pool);
        assert_eq!(fs::read_dir(&scratch.path)?.count(), 0);
        Ok(())
    }

    #[test]
    fn external_pixel_merge_counts_across_multiple_passes() -> Result<()> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&parent)?;
        let scratch = Guard::scratch(&parent)?;
        let mut sorter = sort::Sorter::new(&scratch.path, 1, 4);
        let mut expected = std::collections::BTreeMap::new();
        for i in 0..1100 {
            let (a, b) = (i % 17, i % 7);
            sorter.push(a, b)?;
            *expected.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
        let mut merged = sorter.finish()?;
        for ((a, b), count) in expected {
            assert_eq!(
                merged.next()?,
                Some(sort::Pixel {
                    bin1: a,
                    bin2: b,
                    count
                })
            );
        }
        assert_eq!(merged.next()?, None);
        Ok(())
    }
}
