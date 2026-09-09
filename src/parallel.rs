//! Ordered, bounded multi-producer writing. Submit success means queued, not
//! durable: finish is mandatory. Input batches are explicit shard boundaries.
use crate::*;
use std::{
    collections::{BTreeMap, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug)]
pub struct ParallelOptions {
    pub workers: usize,
    /// Maximum queued out-of-order batches; one additional slot is reserved
    /// for the next expected batch to prevent queue-induced ordering deadlock.
    pub queue_capacity: usize,
    /// Maximum owned input allocation per batch (records, strings, offsets).
    /// This bounds input buffering, not allocator/Parquet workspace or RSS.
    pub max_batch_bytes: usize,
}
impl Default for ParallelOptions {
    fn default() -> Self {
        Self {
            workers: 2,
            queue_capacity: 4,
            max_batch_bytes: 64 * 1024 * 1024,
        }
    }
}

pub(crate) struct Shard {
    pub kind: Kind,
    pub contigs: Vec<Contig>,
    pub staging: PathBuf,
    pub index: usize,
    pub columns: Option<ColumnBatch>,
    pub pairs: Vec<Pair>,
    pub concat: Vec<Alignment>,
    pub concats: [u64; 2],
}
impl Shard {
    pub fn run(&self) -> Result<Counts> {
        let mut frame = match &self.columns {
            Some(ColumnBatch::Pairs(b)) => b.as_view().frame(&self.contigs)?,
            Some(ColumnBatch::Concat(b)) => b.as_view().frame(&self.contigs)?,
            None => match self.kind {
                Kind::Pairs => pair_frame(&self.pairs, &self.contigs)?,
                Kind::Concat => concat_frame(&self.concat, &self.contigs)?,
            },
        };
        store_frame(self.kind, &self.staging, self.index, &mut frame, self.concats)
    }
}
pub(crate) fn store_frame(
    kind: Kind,
    staging: &Path,
    index: usize,
    frame: &mut DataFrame,
    concats: [u64; 2],
) -> Result<Counts> {
    let mq = if kind == Kind::Pairs {
        "mapq"
    } else {
        "mapping_quality"
    };
    let mut q1 = frame.filter(&frame.column(mq)?.u8()?.gt_eq(1))?;
    ParquetWriter::new(File::create(
        staging.join(format!("q0/{}.parquet", index)),
    )?)
    .finish(frame)?;
    if q1.height() > 0 {
        ParquetWriter::new(File::create(
            staging.join(format!("q1/{}.parquet", index)),
        )?)
        .finish(&mut q1)?;
    }
    Ok(Counts {
        q0_records: frame.height() as u64,
        q1_records: q1.height() as u64,
        q0_concats: concats[0],
        q1_concats: concats[1],
    })
}
pub(crate) fn add_counts(dst: &mut Counts, src: Counts) {
    dst.q0_records += src.q0_records;
    dst.q1_records += src.q1_records;
    dst.q0_concats += src.q0_concats;
    dst.q1_concats += src.q1_concats;
}
type Job = (Shard, mpsc::SyncSender<Result<Counts>>);
pub(crate) struct Executor {
    sender: Option<mpsc::SyncSender<Job>>,
    workers: Vec<JoinHandle<()>>,
    pending: VecDeque<mpsc::Receiver<Result<Counts>>>,
    limit: usize,
}
impl Executor {
    fn new(n: usize, shared: Option<Arc<Shared>>) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Job>(n);
        let rx = Arc::new(Mutex::new(rx));
        let mut pool = Self {
            sender: Some(tx),
            workers: vec![],
            pending: VecDeque::new(),
            limit: n,
        };
        for i in 0..n {
            let rx = rx.clone();
            let shared = shared.clone();
            pool.workers.push(
                thread::Builder::new()
                    .name(format!("pqs-encode-{i}"))
                    .spawn(move || loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok((job, reply)) = job else { break };
                        let result =
                            catch_unwind(AssertUnwindSafe(|| job.run())).unwrap_or_else(|_| {
                                Err(anyhow::anyhow!("PQS encoding worker panicked"))
                            });
                        if let (Err(e), Some(shared)) = (&result, &shared) {
                            let mut state = shared.state.lock().unwrap();
                            if state.error.is_none() {
                                state.error = Some(format!("{e:#}"));
                            }
                            shared.changed.notify_all();
                        }
                        let _ = reply.send(result);
                    })?,
            );
        }
        Ok(pool)
    }
    fn collect_one(&mut self, counts: &mut Counts) -> Result<()> {
        if let Some(rx) = self.pending.pop_front() {
            add_counts(counts, rx.recv().context("encoding worker disconnected")??);
        }
        Ok(())
    }
    pub fn submit(&mut self, job: Shard, counts: &mut Counts) -> Result<()> {
        if self.pending.len() == self.limit {
            self.collect_one(counts)?;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .context("encoder closed")?
            .send((job, tx))
            .map_err(|_| anyhow::anyhow!("encoding workers disconnected"))?;
        self.pending.push_back(rx);
        Ok(())
    }
    pub fn drain(&mut self, counts: &mut Counts) -> Result<()> {
        while !self.pending.is_empty() {
            self.collect_one(counts)?;
        }
        Ok(())
    }
}
impl Drop for Executor {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Writer {
    /// Internal opt-in: keep the ordered writer/metadata owner on the caller
    /// while up to `workers` owned shards are encoded and written concurrently.
    pub(crate) fn parallel_encoding(&mut self, workers: usize) -> Result<()> {
        self.active()?;
        ensure!(workers > 0, "encoding workers must be positive");
        ensure!(self.executor.is_none(), "encoding workers already configured");
        self.executor = Some(Executor::new(workers, None)?);
        Ok(())
    }
}

enum Input {
    Pairs(Vec<Pair>),
    Reads(Vec<Alignment>, Vec<usize>),
}
impl Input {
    fn bytes(&self) -> usize {
        match self {
            Self::Pairs(rows) => rows
                .capacity()
                .saturating_mul(std::mem::size_of::<Pair>())
                .saturating_add(
                    rows.iter()
                        .fold(0usize, |n, r| n.saturating_add(r.read_id.capacity())),
                ),
            Self::Reads(rows, offsets) => rows
                .capacity()
                .saturating_mul(std::mem::size_of::<Alignment>())
                .saturating_add(
                    offsets
                        .capacity()
                        .saturating_mul(std::mem::size_of::<usize>()),
                )
                .saturating_add(
                    rows.iter()
                        .fold(0usize, |n, r| n.saturating_add(r.filter_reason.capacity())),
                ),
        }
    }
}
struct State {
    queue: BTreeMap<u64, Input>,
    next: u64,
    closed: bool,
    aborted: bool,
    error: Option<String>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    options: ParallelOptions,
    kind: Kind,
}
/// Cloneable, thread-safe producer. Sequence numbers must be unique and
/// contiguous from zero. A blocked submit owns caller input until admitted.
/// Schedule the next expected batch independently of blocked future batches.
#[derive(Clone)]
pub struct Producer {
    shared: Arc<Shared>,
}
impl Producer {
    fn submit(&self, sequence: u64, input: Input) -> Result<()> {
        ensure!(sequence < u64::MAX, "batch sequence must be below u64::MAX");
        ensure!(
            input.bytes() <= self.shared.options.max_batch_bytes,
            "batch exceeds max_batch_bytes; split between complete reads or increase limit"
        );
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if let Some(e) = &s.error {
                bail!("{e}");
            }
            ensure!(
                !s.closed && !s.aborted,
                "parallel writer is closed or aborted"
            );
            ensure!(
                sequence >= s.next && !s.queue.contains_key(&sequence),
                "duplicate or already consumed batch sequence"
            );
            if sequence == s.next || s.queue.len() < self.shared.options.queue_capacity {
                break;
            }
            s = self.shared.changed.wait(s).unwrap();
        }
        s.queue.insert(sequence, input);
        self.shared.changed.notify_all();
        Ok(())
    }
    pub fn write_pairs(&self, sequence: u64, rows: Vec<Pair>) -> Result<()> {
        ensure!(self.shared.kind == Kind::Pairs, "requires pairs writer");
        self.submit(sequence, Input::Pairs(rows))
    }
    pub fn write_reads(
        &self,
        sequence: u64,
        rows: Vec<Alignment>,
        offsets: Vec<usize>,
    ) -> Result<()> {
        ensure!(self.shared.kind == Kind::Concat, "requires concat writer");
        self.submit(sequence, Input::Reads(rows, offsets))
    }
}
/// Owns publication and worker lifetime. Producers may outlive this object;
/// after finish/drop they return errors. No method publishes partial output.
pub struct ParallelWriter {
    shared: Arc<Shared>,
    coordinator: Option<JoinHandle<Result<Counts>>>,
}
impl ParallelWriter {
    pub fn create(
        path: impl AsRef<Path>,
        kind: Kind,
        contigs: Vec<Contig>,
        chunk_size: usize,
        options: ParallelOptions,
    ) -> Result<Self> {
        ensure!(
            options.workers > 0 && options.queue_capacity > 0 && options.max_batch_bytes > 0,
            "workers, queue_capacity and max_batch_bytes must be positive"
        );
        let mut writer = Writer::create(path, kind, contigs, chunk_size)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: BTreeMap::new(),
                next: 0,
                closed: false,
                aborted: false,
                error: None,
            }),
            changed: Condvar::new(),
            options,
            kind,
        });
        writer.executor = Some(Executor::new(options.workers, Some(shared.clone()))?);
        let state = shared.clone();
        let coordinator =
            thread::Builder::new()
                .name("pqs-coordinate".into())
                .spawn(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| coordinate(&mut writer, &state)))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("PQS coordinator panicked")));
                    // Wake blocked submitters before joining workers and cleanup.
                    {
                        let mut s = state.state.lock().unwrap();
                        s.closed = true;
                        if let Err(e) = &result {
                            s.error = Some(format!("{e:#}"));
                        }
                        s.queue.clear();
                        state.changed.notify_all();
                    }
                    drop(writer);
                    result
                })?;
        Ok(Self {
            shared,
            coordinator: Some(coordinator),
        })
    }
    pub fn producer(&self) -> Producer {
        Producer {
            shared: self.shared.clone(),
        }
    }
    /// Call after all intended submissions have returned. Gaps fail instead
    /// of waiting indefinitely; worker errors prevent publication.
    pub fn finish(&mut self) -> Result<Counts> {
        let handle = self
            .coordinator
            .take()
            .context("parallel writer already finished")?;
        {
            let mut s = self.shared.state.lock().unwrap();
            s.closed = true;
            self.shared.changed.notify_all();
        }
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("coordinator thread panicked"))?
    }
}
impl Drop for ParallelWriter {
    fn drop(&mut self) {
        if let Some(handle) = self.coordinator.take() {
            {
                let mut s = self.shared.state.lock().unwrap();
                s.aborted = true;
                self.shared.changed.notify_all();
            }
            let _ = handle.join();
        }
    }
}
fn coordinate(writer: &mut Writer, shared: &Shared) -> Result<Counts> {
    loop {
        let input = {
            let mut s = shared.state.lock().unwrap();
            loop {
                ensure!(!s.aborted, "parallel writer aborted");
                if let Some(e) = &s.error {
                    bail!("{e}");
                }
                let next = s.next;
                if let Some(input) = s.queue.remove(&next) {
                    s.next += 1;
                    shared.changed.notify_all();
                    break Some(input);
                }
                if s.closed {
                    ensure!(s.queue.is_empty(), "missing batch sequence {}", s.next);
                    break None;
                }
                s = shared.changed.wait(s).unwrap();
            }
        };
        match input {
            Some(Input::Pairs(rows)) => writer.write_pairs_owned(rows)?,
            Some(Input::Reads(rows, offsets)) => writer.write_reads_owned(rows, &offsets)?,
            None => return writer.finish(),
        }
        // A bounded batch is also a shard boundary; never accumulate an
        // unbounded number of large strings across small submissions.
        writer.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_blocks_future_batches_reserves_expected_slot_and_wakes_on_abort() {
        // Isolate admission from coordinator timing: no consumer runs here.
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: BTreeMap::new(),
                next: 0,
                closed: false,
                aborted: false,
                error: None,
            }),
            changed: Condvar::new(),
            options: ParallelOptions {
                workers: 1,
                queue_capacity: 1,
                max_batch_bytes: 1,
            },
            kind: Kind::Pairs,
        });
        let p = Producer {
            shared: shared.clone(),
        };
        p.write_pairs(1, vec![]).unwrap();
        let future = p.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx
                .send(future.write_pairs(2, vec![]).is_err())
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            done_rx.recv_timeout(std::time::Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(shared.state.lock().unwrap().queue.len(), 1);
        p.write_pairs(0, vec![]).unwrap();
        assert_eq!(shared.state.lock().unwrap().queue.len(), 2);
        shared.state.lock().unwrap().aborted = true;
        shared.changed.notify_all();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap());
        handle.join().unwrap();
    }
}
