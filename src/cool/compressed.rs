//! Compress pixel chunks on workers; only the caller accesses HDF5.
use anyhow::{ensure, Context, Result};
use flate2::{write::ZlibEncoder, Compression};
use hdf5::{filters::Filter, Dataset};
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};

pub(super) type Columns = [Vec<i64>; 3];
struct Job {
    offset: usize,
    chunk: usize,
    columns: Columns,
}
struct Encoded {
    offset: usize,
    length: usize,
    blocks: [Vec<u8>; 3],
    columns: Columns,
}

fn encode(job: Job) -> Result<Encoded> {
    let length = job.columns[0].len();
    ensure!(
        length > 0 && length <= job.chunk && job.columns.iter().all(|c| c.len() == length),
        "invalid pixel compression chunk"
    );
    let mut shuffled = vec![0u8; job.chunk.checked_mul(8).context("pixel chunk too large")?];
    let mut blocks: [Vec<u8>; 3] = Default::default();
    for (values, output) in job.columns.iter().zip(&mut blocks) {
        // HDF5 shuffle groups the bytes of each native Int64 by byte position.
        // Untouched tail entries remain zero: raw chunk writes require the full
        // physical chunk even when the logical final extent is shorter.
        for (i, value) in values.iter().enumerate() {
            for (byte, value) in value.to_ne_bytes().into_iter().enumerate() {
                shuffled[byte * job.chunk + i] = value;
            }
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&shuffled)?;
        *output = encoder.finish()?;
    }
    Ok(Encoded {
        offset: job.offset,
        length,
        blocks,
        columns: job.columns,
    })
}

struct Pool {
    sender: Option<mpsc::SyncSender<Job>>,
    results: mpsc::Receiver<Result<Encoded>>,
    workers: Vec<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    pending: usize,
    limit: usize,
}
impl Pool {
    fn new(threads: usize) -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(threads);
        let jobs = Arc::new(Mutex::new(jobs));
        let (complete, results) = mpsc::channel();
        let mut pool = Self {
            sender: Some(sender),
            results,
            workers: vec![],
            cancelled: Arc::new(AtomicBool::new(false)),
            pending: 0,
            limit: threads,
        };
        for index in 0..threads {
            let (jobs, complete, cancelled) = (
                Arc::clone(&jobs),
                complete.clone(),
                Arc::clone(&pool.cancelled),
            );
            pool.workers.push(
                thread::Builder::new()
                    .name(format!("cool-deflate-{index}"))
                    .spawn(move || {
                        while !cancelled.load(Ordering::Relaxed) {
                            let job = match jobs.lock() {
                                Ok(jobs) => jobs.recv(),
                                Err(_) => {
                                    cancelled.store(true, Ordering::Relaxed);
                                    let _ = complete
                                        .send(Err(anyhow::anyhow!("compression queue poisoned")));
                                    break;
                                }
                            };
                            let Ok(job) = job else { break };
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    encode(job)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("pixel compressor panicked"))
                                });
                            if result.is_err() {
                                cancelled.store(true, Ordering::Relaxed);
                            }
                            if complete.send(result).is_err() {
                                break;
                            }
                        }
                    })
                    .context("cannot start pixel compression worker")?,
            );
        }
        Ok(pool)
    }
    fn submit(&mut self, job: Job) -> Result<()> {
        // This credits both running jobs and completed buffers, so even the
        // result channel contains at most `limit` chunks. Sends cannot deadlock
        // with workers waiting for the caller to consume their results.
        ensure!(self.pending < self.limit, "pixel compression queue is full");
        self.sender
            .as_ref()
            .unwrap()
            .send(job)
            .context("pixel compression workers stopped")?;
        self.pending += 1;
        Ok(())
    }
    fn receive(&mut self) -> Result<Encoded> {
        let result = self
            .results
            .recv()
            .context("pixel compression worker disconnected")?;
        self.pending -= 1;
        result
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

pub(super) struct Writer {
    datasets: [Dataset; 3],
    chunk: usize,
    threads: usize,
    next: usize,
    extent: usize,
    sealed: bool,
    pool: Option<Pool>,
}
impl Writer {
    pub fn new(datasets: [Dataset; 3], chunk: usize, threads: usize) -> Result<Self> {
        ensure!(
            chunk > 0 && threads > 0,
            "pixel chunk size and threads must be positive"
        );
        for dataset in &datasets {
            ensure!(
                dataset.ndim() == 1
                    && dataset.size() == 0
                    && dataset.chunk() == Some(vec![chunk])
                    && dataset.dtype()?.is::<i64>()
                    && dataset.filters() == [Filter::Shuffle, Filter::Deflate(6)],
                "pixel compression requires empty native Int64, shuffle/deflate-6 chunked datasets"
            );
        }
        Ok(Self {
            datasets,
            chunk,
            threads,
            next: 0,
            extent: 0,
            sealed: false,
            pool: None,
        })
    }
    pub fn append(&mut self, columns: &mut Columns) -> Result<()> {
        let length = columns[0].len();
        ensure!(
            columns.iter().all(|c| c.len() == length),
            "pixel column lengths differ"
        );
        if length == 0 {
            return Ok(());
        }
        ensure!(!self.sealed, "cannot append after a partial pixel chunk");
        let end = self
            .next
            .checked_add(length)
            .context("pixel extent exceeds address space")?;
        // Small chunks and a single short output use ordinary HDF5 writes.
        // One worker also preserves the existing serial path and backend.
        if self.threads == 1 || self.chunk < 4096 || (self.pool.is_none() && length < self.chunk) {
            for (dataset, values) in self.datasets.iter().zip(columns) {
                super::write::append(dataset, values)?;
                values.clear();
            }
            self.next = end;
            self.extent = end;
            return Ok(());
        }
        ensure!(
            length <= self.chunk && self.next.is_multiple_of(self.chunk),
            "unaligned pixel compression chunk"
        );
        if self.pool.is_none() {
            self.pool = Some(Pool::new(self.threads)?);
        }
        let pool = self.pool.as_mut().unwrap();
        let mut spare = if pool.pending == pool.limit {
            let encoded = pool.receive()?;
            self.write(encoded)?
        } else {
            std::array::from_fn(|_| Vec::with_capacity(self.chunk))
        };
        std::mem::swap(columns, &mut spare);
        self.pool.as_mut().unwrap().submit(Job {
            offset: self.next,
            chunk: self.chunk,
            columns: spare,
        })?;
        self.next = end;
        self.sealed = length < self.chunk;
        Ok(())
    }
    fn write(&mut self, mut encoded: Encoded) -> Result<Columns> {
        let end = encoded
            .offset
            .checked_add(encoded.length)
            .context("pixel extent overflow")?;
        if end > self.extent {
            for dataset in &self.datasets {
                dataset.resize((end,))?;
            }
            self.extent = end;
        }
        let offset = [u64::try_from(encoded.offset)?];
        for (dataset, bytes) in self.datasets.iter().zip(&encoded.blocks) {
            // SAFETY: constructor checked rank, chunking, native i64 type and
            // exact filter order. Each aligned chunk is written once, after
            // extending the dataset. encode supplied shuffle + zlib including
            // full zero padding; mask 0 means neither filter was skipped. The
            // borrowed buffer and offset live for the synchronous HDF5 call.
            let status = hdf5::sync::sync(|| unsafe {
                hdf5_sys::h5d::H5Dwrite_chunk(
                    dataset.id(),
                    hdf5_sys::h5p::H5P_DEFAULT,
                    0,
                    offset.as_ptr(),
                    bytes.len(),
                    bytes.as_ptr().cast(),
                )
            });
            ensure!(
                status >= 0,
                "HDF5 failed writing compressed pixel chunk at {}",
                encoded.offset
            );
        }
        for values in &mut encoded.columns {
            values.clear();
        }
        Ok(encoded.columns)
    }
    pub fn finish(mut self) -> Result<()> {
        while self.pool.as_ref().is_some_and(|p| p.pending > 0) {
            let encoded = self.pool.as_mut().unwrap().receive()?;
            self.write(encoded)?;
        }
        self.pool = None; // Join before the caller closes or publishes the HDF5 file.
        ensure!(self.extent == self.next, "incomplete pixel output");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cool::Guard;
    use std::{fs, path::Path};

    fn datasets(file: &hdf5::File, chunk: usize) -> Result<[Dataset; 3]> {
        ["a", "b", "c"]
            .map(|name| {
                file.new_dataset::<i64>()
                    .shape((0..,))
                    .chunk((chunk,))
                    .shuffle()
                    .deflate(6)
                    .create(name)
                    .map_err(Into::into)
            })
            .into_iter()
            .collect::<Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| anyhow::anyhow!("expected three datasets"))
    }
    fn scratch() -> Result<Guard> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output");
        fs::create_dir_all(&parent)?;
        Guard::scratch(&parent)
    }

    #[test]
    fn compressed_chunks_roundtrip_full_partial_and_out_of_order() -> Result<()> {
        let root = scratch()?;
        let chunk = 4096;
        for size in [0, 1, chunk, 2 * chunk, 3 * chunk + 17] {
            let expected: Columns = std::array::from_fn(|column| {
                (0..size)
                    .map(|i| match column {
                        0 => i64::MAX - i as i64,
                        1 => i64::MIN + i as i64,
                        _ => (1i64 << 40) + i as i64 * 17,
                    })
                    .collect()
            });
            for threads in [1, 4] {
                let path = root.path.join(format!("{size}-{threads}.h5"));
                let file = hdf5::File::create(&path)?;
                let mut writer = Writer::new(datasets(&file, chunk)?, chunk, threads)?;
                for start in (0..size).step_by(chunk) {
                    let mut columns = expected
                        .each_ref()
                        .map(|values| values[start..size.min(start + chunk)].to_vec());
                    writer.append(&mut columns)?;
                    assert!(columns.iter().all(Vec::is_empty));
                    assert!(writer.pool.as_ref().is_none_or(|p| p.pending <= threads));
                }
                assert_eq!(writer.pool.is_some(), threads > 1 && size >= chunk);
                writer.finish()?;
                file.close()?;
                let file = hdf5::File::open(path)?;
                for (name, expected) in ["a", "b", "c"].into_iter().zip(&expected) {
                    assert_eq!(file.dataset(name)?.read_raw::<i64>()?, *expected);
                }
            }
        }
        // Force the short last chunk to arrive before earlier full chunks.
        let file = hdf5::File::create(root.path.join("out-of-order.h5"))?;
        let mut writer = Writer::new(datasets(&file, chunk)?, chunk, 4)?;
        for (offset, length) in [(chunk, 17), (0, chunk)] {
            writer.write(encode(Job {
                offset,
                chunk,
                columns: std::array::from_fn(|_| {
                    (offset..offset + length).map(|i| i as i64).collect()
                }),
            })?)?;
        }
        writer.next = chunk + 17;
        writer.finish()?;
        assert_eq!(
            file.dataset("c")?.read_raw::<i64>()?,
            (0..chunk + 17).map(|i| i as i64).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn compression_backpressure_failures_and_cancellation() -> Result<()> {
        let mut pool = Pool::new(2)?;
        for offset in [0, 4096] {
            pool.submit(Job {
                offset,
                chunk: 4096,
                columns: std::array::from_fn(|_| vec![7; 4096]),
            })?;
        }
        assert!(pool
            .submit(Job {
                offset: 8192,
                chunk: 4096,
                columns: Default::default()
            })
            .is_err());
        pool.receive()?;
        pool.receive()?;
        pool.submit(Job {
            offset: 8192,
            chunk: 4096,
            columns: Default::default(),
        })?;
        assert!(pool
            .receive()
            .err()
            .context("worker accepted invalid chunk")?
            .to_string()
            .contains("invalid pixel"));
        drop(pool);
        // Dropping with queued work joins workers without waiting for results.
        let mut pool = Pool::new(2)?;
        for offset in [0, 65_536] {
            pool.submit(Job {
                offset,
                chunk: 65_536,
                columns: std::array::from_fn(|_| vec![7; 65_536]),
            })?;
        }
        drop(pool);

        let root = scratch()?;
        let path = root.path.join("read-only.h5");
        let file = hdf5::File::create(&path)?;
        drop(datasets(&file, 4096)?);
        file.close()?;
        let file = hdf5::File::open(&path)?;
        let columns = [file.dataset("a")?, file.dataset("b")?, file.dataset("c")?];
        let mut writer = Writer::new(columns, 4096, 4)?;
        writer.append(&mut std::array::from_fn(|_| vec![7; 4096]))?;
        assert!(writer.finish().is_err()); // HDF5 write failure also joins workers.
        Ok(())
    }
}
