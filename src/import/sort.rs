//! Bounded binary runs, merged in bounded fan-in passes. No subprocess/SQLite.
use super::Record;
use anyhow::{Context, Result};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

const FAN_IN: usize = 32;
static SERIAL: AtomicU64 = AtomicU64::new(0);

pub(super) struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new(parent: &Path) -> Result<Self> {
        loop {
            let path = parent.join(format!(
                ".pqsio-import-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).context("cannot create import scratch directory"),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compare(a: &Record, b: &Record) -> Ordering {
    (&a.rg, &a.name, a.mate, a.row.read_start, a.ordinal).cmp(&(
        &b.rg,
        &b.name,
        b.mate,
        b.row.read_start,
        b.ordinal,
    ))
}

fn write_record(w: &mut impl Write, record: &Record) -> Result<()> {
    for text in [&record.rg, &record.name] {
        w.write_all(&u32::try_from(text.len())?.to_le_bytes())?;
        w.write_all(text.as_bytes())?;
    }
    let r = &record.row;
    w.write_all(&record.ordinal.to_le_bytes())?;
    for n in [r.read_length, r.read_start, r.read_end, r.chrom] {
        w.write_all(&n.to_le_bytes())?;
    }
    for n in [r.start, r.end] {
        w.write_all(&n.to_le_bytes())?;
    }
    w.write_all(&r.identity.to_le_bytes())?;
    w.write_all(&[r.strand, r.mapping_quality, record.mate])?;
    Ok(())
}

fn read_record(r: &mut impl BufRead) -> Result<Option<Record>> {
    if r.fill_buf()?.is_empty() {
        return Ok(None);
    }
    fn bytes<const N: usize>(r: &mut impl Read) -> Result<[u8; N]> {
        let mut b = [0; N];
        r.read_exact(&mut b)?;
        Ok(b)
    }
    fn string(r: &mut impl Read) -> Result<String> {
        let len = u32::from_le_bytes(bytes(r)?) as usize;
        let mut b = vec![0; len];
        r.read_exact(&mut b)?;
        Ok(String::from_utf8(b)?)
    }
    let rg = string(r)?;
    let name = string(r)?;
    let ordinal = u64::from_le_bytes(bytes(r)?);
    let read_length = u32::from_le_bytes(bytes(r)?);
    let read_start = u32::from_le_bytes(bytes(r)?);
    let read_end = u32::from_le_bytes(bytes(r)?);
    let chrom = u32::from_le_bytes(bytes(r)?);
    let start = u64::from_le_bytes(bytes(r)?);
    let end = u64::from_le_bytes(bytes(r)?);
    let identity = f32::from_le_bytes(bytes(r)?);
    let [strand, mapping_quality, mate] = bytes(r)?;
    Ok(Some(Record {
        rg,
        name,
        ordinal,
        mate,
        row: crate::Alignment {
            read_idx: 0,
            read_length,
            read_start,
            read_end,
            chrom,
            start,
            end,
            identity,
            strand,
            mapping_quality,
            filter_reason: "pass".into(),
        },
    }))
}

pub(super) struct Sorter {
    directory: PathBuf,
    limit: usize,
    bytes: usize,
    rows: Vec<Record>,
    runs: Vec<PathBuf>,
    serial: usize,
}
impl Sorter {
    pub fn new(directory: &Path, limit: usize) -> Self {
        Self {
            directory: directory.into(),
            limit,
            bytes: 0,
            rows: vec![],
            runs: vec![],
            serial: 0,
        }
    }
    fn path(&mut self) -> PathBuf {
        self.serial += 1;
        self.directory.join(format!("run-{}", self.serial))
    }
    pub fn push(&mut self, row: Record) -> Result<()> {
        self.bytes += std::mem::size_of::<Record>() + row.name.len() + row.rg.len() + 4;
        self.rows.push(row);
        if self.bytes >= self.limit {
            self.flush()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        self.rows.sort_unstable_by(compare);
        let path = self.path();
        let mut output = BufWriter::with_capacity(256 * 1024, File::create(&path)?);
        for row in &self.rows {
            write_record(&mut output, row)?;
        }
        output.flush()?;
        self.rows.clear();
        self.bytes = 0;
        self.runs.push(path);
        Ok(())
    }
    pub fn finish(mut self) -> Result<Merged> {
        self.flush()?;
        // Release the parsing buffer before merge/output buffers are allocated.
        self.rows = Vec::new();
        while self.runs.len() > FAN_IN {
            let old = std::mem::take(&mut self.runs);
            for paths in old.chunks(FAN_IN) {
                let path = self.path();
                let mut output = BufWriter::with_capacity(256 * 1024, File::create(&path)?);
                let mut merged = Merged::new(paths)?;
                while let Some(row) = merged.next()? {
                    write_record(&mut output, &row)?;
                }
                output.flush()?;
                drop(merged);
                for path in paths {
                    fs::remove_file(path)?;
                }
                self.runs.push(path);
            }
        }
        Merged::new(&self.runs)
    }
}

struct Head {
    row: Record,
    source: usize,
}
impl PartialEq for Head {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Head {}
impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Head {
    fn cmp(&self, other: &Self) -> Ordering {
        compare(&other.row, &self.row).then_with(|| other.source.cmp(&self.source))
    }
}
pub(super) struct Merged {
    readers: Vec<BufReader<File>>,
    heap: BinaryHeap<Head>,
}
impl Merged {
    fn new(paths: &[PathBuf]) -> Result<Self> {
        let mut merged = Self {
            readers: vec![],
            heap: BinaryHeap::new(),
        };
        for (source, path) in paths.iter().enumerate() {
            let mut reader = BufReader::with_capacity(64 * 1024, File::open(path)?);
            if let Some(row) = read_record(&mut reader)? {
                merged.heap.push(Head { row, source });
            }
            merged.readers.push(reader);
        }
        Ok(merged)
    }
    pub fn next(&mut self) -> Result<Option<Record>> {
        let Some(Head { row, source }) = self.heap.pop() else {
            return Ok(None);
        };
        if let Some(row) = read_record(&mut self.readers[source])? {
            self.heap.push(Head { row, source });
        }
        Ok(Some(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_merge_passes_preserve_group_order_and_ties() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&root)?;
        let scratch = Scratch::new(&root)?;
        let mut sort = Sorter::new(&scratch.0, 1);
        for ordinal in 0..1050 {
            sort.push(Record {
                name: format!("r{}", ordinal % 7),
                rg: format!("g{}", ordinal % 3),
                ordinal,
                mate: (ordinal % 2) as u8,
                row: crate::Alignment {
                    read_idx: 0,
                    read_length: 100,
                    read_start: (ordinal % 5) as u32,
                    read_end: 80,
                    strand: b'-',
                    chrom: 2,
                    start: 1 << 33,
                    end: (1 << 33) + 10,
                    mapping_quality: 30,
                    identity: 0.9,
                    filter_reason: "pass".into(),
                },
            })?;
        }
        let mut merge = sort.finish()?;
        let mut previous = None;
        let mut seen = std::collections::HashSet::new();
        while let Some(row) = merge.next()? {
            if let Some(previous) = &previous {
                assert!(compare(previous, &row).is_lt());
            }
            assert!(seen.insert(row.ordinal));
            assert_eq!(row.row.start, 1 << 33);
            previous = Some(row);
        }
        assert_eq!(seen.len(), 1050);
        Ok(())
    }
}
