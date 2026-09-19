//! Sorted, aggregated pixel runs with bounded fan-in merging.
use super::sort_pool::Pool;
use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Pixel {
    pub bin1: i64,
    pub bin2: i64,
    pub count: i64,
}
impl Pixel {
    fn key(self) -> (i64, i64) {
        (self.bin1, self.bin2)
    }
    fn write(self, writer: &mut impl Write) -> Result<()> {
        for value in [self.bin1, self.bin2, self.count] {
            writer.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }
    fn read(reader: &mut impl BufRead) -> Result<Option<Self>> {
        if reader.fill_buf()?.is_empty() {
            return Ok(None);
        }
        let mut values = [0; 3];
        for value in &mut values {
            let mut bytes = [0; 8];
            reader.read_exact(&mut bytes)?;
            *value = i64::from_le_bytes(bytes);
        }
        Ok(Some(Self {
            bin1: values[0],
            bin2: values[1],
            count: values[2],
        }))
    }
}

pub(super) struct Sorter {
    directory: PathBuf,
    limit: usize,
    rows: Vec<Pixel>,
    runs: Vec<PathBuf>,
    serial: usize,
    threads: usize,
    pool: Option<Pool>,
}
impl Sorter {
    pub fn new(directory: &Path, limit: usize, threads: usize) -> Self {
        Self {
            directory: directory.into(),
            limit,
            rows: vec![],
            runs: vec![],
            serial: 0,
            threads,
            pool: None,
        }
    }
    fn path(&mut self) -> PathBuf {
        self.serial += 1;
        self.directory.join(format!("pixels-{}", self.serial))
    }
    pub fn push(&mut self, bin1: i64, bin2: i64) -> Result<()> {
        // Delay spilling until another record proves this is not a single run.
        if self.rows.len() == self.limit {
            self.flush()?;
        }
        self.rows.push(Pixel {
            bin1: bin1.min(bin2),
            bin2: bin1.max(bin2),
            count: 1,
        });
        Ok(())
    }
    pub fn extend(&mut self, rows: Vec<Pixel>) -> Result<()> {
        let mut remaining = rows.as_slice();
        while !remaining.is_empty() {
            if self.rows.len() == self.limit {
                self.flush()?;
            }
            let size = remaining.len().min(self.limit - self.rows.len());
            self.rows.extend_from_slice(&remaining[..size]);
            remaining = &remaining[size..];
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let path = self.path();
        if self.threads > 1 {
            if self.pool.is_none() {
                self.pool = Some(Pool::new(self.threads)?);
            }
            self.pool
                .as_mut()
                .unwrap()
                .submit(std::mem::take(&mut self.rows), path.clone())?;
        } else {
            write_run(&mut self.rows, &path)?;
            self.rows.clear();
        }
        self.runs.push(path);
        Ok(())
    }
    pub fn finish(mut self) -> Result<Merged> {
        if self.runs.is_empty() {
            return Merged::memory(self.rows, self.threads);
        }
        self.flush()?;
        self.rows = vec![];
        if let Some(pool) = &mut self.pool {
            pool.finish()?;
        }
        self.pool = None; // Join workers before merging or removing scratch files.
        while self.runs.len() > 32 {
            let old = std::mem::take(&mut self.runs);
            for paths in old.chunks(32) {
                let path = self.path();
                let mut writer = BufWriter::with_capacity(256 * 1024, File::create(&path)?);
                let mut merged = Merged::new(paths)?;
                while let Some(row) = merged.next()? {
                    row.write(&mut writer)?;
                }
                writer.flush()?;
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

pub(super) struct Merged {
    memory: Option<std::vec::IntoIter<Pixel>>,
    readers: Vec<Run>,
    heap: BinaryHeap<Reverse<(Pixel, usize)>>,
}
impl Merged {
    fn memory(mut rows: Vec<Pixel>, threads: usize) -> Result<Self> {
        // Tiny runs stay on the caller; large runs sort disjoint slices in parallel.
        let workers = threads.min(rows.len().div_ceil(65_536)).max(1);
        if workers == 1 {
            let count = aggregate(&mut rows)?;
            rows.truncate(count);
            rows.shrink_to_fit();
            return Ok(Self {
                memory: Some(rows.into_iter()),
                readers: vec![],
                heap: BinaryHeap::new(),
            });
        }
        let chunk = rows.len().div_ceil(workers);
        let mut parts = std::thread::scope(|scope| -> Result<Vec<Range<usize>>> {
            let mut handles = vec![];
            for (index, rows) in rows.chunks_mut(chunk).enumerate() {
                handles.push(
                    std::thread::Builder::new()
                        .name(format!("cool-memory-sort-{index}"))
                        .spawn_scoped(scope, move || {
                            aggregate(rows).map(|count| index * chunk..index * chunk + count)
                        })?,
                );
            }
            // Join every worker before returning any failure.
            let results: Vec<_> = handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        Err(anyhow::anyhow!("Cooler memory sort worker panicked"))
                    })
                })
                .collect();
            results.into_iter().collect()
        })?;
        // Pack the aggregated prefixes before HDF5 allocates its write buffers.
        // Moving them left in order cannot overwrite a later part's source.
        let mut used = 0;
        for range in &mut parts {
            let length = range.len();
            rows.copy_within(range.clone(), used);
            *range = used..used + length;
            used += length;
        }
        rows.truncate(used);
        rows.shrink_to_fit();
        let rows = Arc::new(rows);
        let readers = parts
            .into_iter()
            .map(|range| Run::Memory {
                rows: Arc::clone(&rows),
                range,
            })
            .collect();
        Self::with_readers(readers)
    }
    fn new(paths: &[PathBuf]) -> Result<Self> {
        Self::with_readers(
            paths
                .iter()
                .map(|path| {
                    Ok(Run::Disk(BufReader::with_capacity(
                        64 * 1024,
                        File::open(path)?,
                    )))
                })
                .collect::<Result<Vec<_>>>()?,
        )
    }
    fn with_readers(readers: Vec<Run>) -> Result<Self> {
        let mut value = Self {
            memory: None,
            readers,
            heap: BinaryHeap::new(),
        };
        for (index, reader) in value.readers.iter_mut().enumerate() {
            if let Some(pixel) = reader.next()? {
                value.heap.push(Reverse((pixel, index)));
            }
        }
        Ok(value)
    }
    fn pop(&mut self) -> Result<Option<Pixel>> {
        let Some(mut head) = self.heap.peek_mut() else {
            return Ok(None);
        };
        let Reverse((pixel, index)) = *head;
        if let Some(next) = self.readers[index].next()? {
            // Replacing the minimum requires one sift-down instead of pop+push.
            *head = Reverse((next, index));
        } else {
            std::collections::binary_heap::PeekMut::pop(head);
        }
        Ok(Some(pixel))
    }
    pub fn next(&mut self) -> Result<Option<Pixel>> {
        if let Some(rows) = &mut self.memory {
            return Ok(rows.next());
        }
        let Some(mut pixel) = self.pop()? else {
            return Ok(None);
        };
        while self
            .heap
            .peek()
            .is_some_and(|Reverse((p, _))| p.key() == pixel.key())
        {
            let other = self.pop()?.unwrap();
            pixel.count = pixel
                .count
                .checked_add(other.count)
                .context("pixel count exceeds Int64")?;
        }
        Ok(Some(pixel))
    }
}

enum Run {
    Disk(BufReader<File>),
    Memory {
        rows: Arc<Vec<Pixel>>,
        range: Range<usize>,
    },
}
impl Run {
    fn next(&mut self) -> Result<Option<Pixel>> {
        match self {
            Self::Disk(reader) => Pixel::read(reader),
            Self::Memory { rows, range } => Ok(range.next().map(|i| rows[i])),
        }
    }
}

pub(super) fn write_run(rows: &mut [Pixel], path: &Path) -> Result<()> {
    let length = aggregate(rows)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, File::create(path)?);
    for row in &rows[..length] {
        row.write(&mut writer)?;
    }
    writer.flush()?;
    Ok(())
}

fn aggregate(rows: &mut [Pixel]) -> Result<usize> {
    rows.sort_unstable_by_key(|r| r.key());
    let mut count = 0;
    for i in 0..rows.len() {
        if count > 0 && rows[i].key() == rows[count - 1].key() {
            rows[count - 1].count = rows[count - 1]
                .count
                .checked_add(rows[i].count)
                .context("pixel count exceeds Int64")?;
        } else {
            rows[count] = rows[i];
            count += 1;
        }
    }
    Ok(count)
}
