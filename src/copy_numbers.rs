//! Optional explicit CN declarations; no record or coordinate transformation.
use crate::metadata::{obj, Value};
use crate::*;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub struct CopyNumbers {
    pub present: bool,
    pub explicit: BTreeMap<String, u64>,
    contigs: HashSet<String>,
}
impl CopyNumbers {
    pub fn effective(&self, contig: &str) -> Result<u64> {
        ensure!(self.contigs.contains(contig), "unknown contig {contig:?}");
        Ok(self.explicit.get(contig).copied().unwrap_or(1))
    }
    pub fn to_value(&self) -> Value {
        obj([
            ("present", Value::Bool(self.present)),
            (
                "explicit",
                Value::Object(
                    self.explicit
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::from(*v)))
                        .collect(),
                ),
            ),
            ("declaration_count", Value::from(self.explicit.len() as u64)),
            ("default", Value::from(1)),
            (
                "contigs",
                Value::List({
                    let mut names: Vec<_> = self.contigs.iter().collect();
                    names.sort();
                    names.into_iter().map(|n| Value::from(n.as_str())).collect()
                }),
            ),
        ])
    }
}
#[derive(Debug)]
pub struct CopyNumberError {
    pub code: &'static str,
    pub file: String,
    pub line: u64,
    pub contig: String,
    pub message: String,
}
impl std::fmt::Display for CopyNumberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{} contig {:?}: {}",
            self.file, self.line, self.contig, self.message
        )
    }
}
impl std::error::Error for CopyNumberError {}

pub(crate) fn read_with_contigs(path: &Path, contigs: &[Contig]) -> Result<CopyNumbers> {
    let file = path.join("cn.info");
    let mut info = CopyNumbers {
        present: false,
        explicit: BTreeMap::new(),
        contigs: contigs.iter().map(|c| c.name.clone()).collect(),
    };
    // A dangling link or non-file is unreadable, not an absent optional file.
    match fs::symlink_metadata(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(info),
        Err(e) => return Err(e.into()),
        Ok(_) => info.present = true,
    }
    for (i, line) in BufReader::new(
        File::open(&file).with_context(|| format!("cannot read {}", file.display()))?,
    )
    .lines()
    .enumerate()
    {
        let line = line.with_context(|| format!("cannot read {}:{}", file.display(), i + 1))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let error = |code, message: &str| CopyNumberError {
            code,
            file: file.display().to_string(),
            line: i as u64 + 1,
            contig: fields[0].into(),
            message: message.into(),
        };
        if fields.len() != 2 {
            return Err(error("CN_FORMAT", "expected exactly '<contig> <CN>'").into());
        }
        let cn = fields[1]
            .parse::<u64>()
            .ok()
            .filter(|v| *v >= 1)
            .ok_or_else(|| {
                error(
                    "CN_VALUE",
                    "CN must be an integer in 1..=18446744073709551615",
                )
            })?;
        if !info.contigs.contains(fields[0]) {
            return Err(error("CN_UNKNOWN_CONTIG", "not present in _contigsizes").into());
        }
        if info.explicit.insert(fields[0].into(), cn).is_some() {
            return Err(error("CN_DUPLICATE", "duplicate declaration").into());
        }
    }
    Ok(info)
}

pub fn read_copy_numbers(path: impl AsRef<Path>) -> Result<CopyNumbers> {
    let path = path.as_ref();
    read_with_contigs(path, &dataset_contigs(path)?)
}
fn dataset_contigs(path: &Path) -> Result<Vec<Contig>> {
    let mut names = HashSet::new();
    fs::read_to_string(path.join("_contigsizes"))?
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(
                fields.len() == 2,
                "_contigsizes:{}: expected two fields",
                i + 1
            );
            ensure!(
                names.insert(fields[0].to_string()),
                "_contigsizes:{}: duplicate contig {:?}",
                i + 1,
                fields[0]
            );
            Ok(Contig {
                name: fields[0].into(),
                length: fields[1].parse().context("invalid _contigsizes length")?,
            })
        })
        .collect()
}
fn check(values: &BTreeMap<String, u64>, contigs: &[Contig]) -> Result<()> {
    for (name, value) in values {
        ensure!(
            !name.starts_with('#')
                && !name.is_empty()
                && !name.chars().any(char::is_whitespace)
                && !name.contains('\0'),
            "cn.info contig {name:?}: unrepresentable name"
        );
        ensure!(
            *value >= 1,
            "cn.info contig {name:?}: CN must be in 1..=18446744073709551615"
        );
        ensure!(
            contigs.iter().any(|c| c.name == *name),
            "cn.info unknown contig {name:?}: not present in _contigsizes"
        );
    }
    Ok(())
}
static NEXT: AtomicU64 = AtomicU64::new(0);
fn replace(path: &Path, values: &BTreeMap<String, u64>) -> Result<()> {
    let dest = path.join("cn.info");
    if let Ok(m) = fs::symlink_metadata(&dest) {
        ensure!(
            !m.file_type().is_symlink(),
            "refusing linked cn.info {}; use a real dataset file",
            dest.display()
        );
    }
    let temp = path.join(format!(
        ".cn.info.{}.{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        for (name, cn) in values {
            writeln!(file, "{name}\t{cn}")?;
        }
        file.flush()?;
        drop(file);
        fs::rename(&temp, &dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("cannot replace {}", dest.display()))
}
fn modify(path: &Path, values: &BTreeMap<String, u64>, update: bool) -> Result<()> {
    // Reject symlinks in every directory component, including directory aliases.
    let mut component_path = PathBuf::new();
    for component in path.components() {
        component_path.push(component);
        ensure!(
            !fs::symlink_metadata(&component_path)?
                .file_type()
                .is_symlink(),
            "refusing symlink dataset path {}; use its real directory",
            component_path.display()
        );
    }
    let contigs = dataset_contigs(path)?;
    check(values, &contigs)?;
    if update && values.is_empty() {
        return Ok(());
    }
    let mut merged = if update {
        read_with_contigs(path, &contigs)?.explicit
    } else {
        BTreeMap::new()
    };
    merged.extend(values.iter().map(|(k, v)| (k.clone(), *v)));
    replace(path, &merged)
}
pub fn set_copy_numbers(path: impl AsRef<Path>, values: &BTreeMap<String, u64>) -> Result<()> {
    modify(path.as_ref(), values, false)
}
pub fn update_copy_numbers(path: impl AsRef<Path>, values: &BTreeMap<String, u64>) -> Result<()> {
    modify(path.as_ref(), values, true)
}
impl Writer {
    /// Explicit setting stage, before finish. Errors poison the staging writer.
    pub fn set_copy_numbers(&mut self, values: &BTreeMap<String, u64>) -> Result<()> {
        self.active()?;
        (|| {
            check(values, &self.contigs)?;
            replace(&self.staging, values)
        })()
        .inspect_err(|_| self.failed = true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_api_and_failed_publication() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/output")
            .join(format!("cn-rust-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let result = (|| -> Result<()> {
            let path = root.join("dataset");
            let contigs = vec![
                Contig {
                    name: "a".into(),
                    length: 100,
                },
                Contig {
                    name: "b".into(),
                    length: 100,
                },
            ];
            let mut writer = Writer::create(&path, Kind::Pairs, contigs.clone(), 1)?;
            writer.set_copy_numbers(&BTreeMap::from([("a".into(), u64::MAX)]))?;
            writer.finish()?;
            let info = read_copy_numbers(&path)?;
            assert!(info.present);
            assert_eq!(info.effective("a")?, u64::MAX);
            assert_eq!(info.effective("b")?, 1);
            assert!(info.effective("unknown").is_err());
            update_copy_numbers(&path, &BTreeMap::from([("b".into(), 1)]))?;
            assert_eq!(read_copy_numbers(&path)?.explicit.len(), 2);
            set_copy_numbers(&path, &BTreeMap::new())?;
            assert!(read_copy_numbers(&path)?.present);
            let output = root.join("failed");
            let staging;
            {
                let mut writer = Writer::create(&output, Kind::Pairs, contigs, 1)?;
                staging = writer.staging.clone();
                fs::create_dir(staging.join("cn.info"))?;
                assert!(writer
                    .set_copy_numbers(&BTreeMap::from([("a".into(), 2)]))
                    .is_err());
                assert!(writer.failed);
                assert!(writer.finish().is_err());
            }
            assert!(!output.exists());
            assert!(!staging.exists());
            Ok(())
        })();
        fs::remove_dir_all(root)?;
        result
    }
}
