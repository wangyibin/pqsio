//! Bounded, persistent sort/spill workers. Completion messages contain no row buffers.
use super::sort::{write_run, Pixel};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};

type Job = (Vec<Pixel>, PathBuf);
pub(super) struct Pool {
    sender: Option<mpsc::SyncSender<Job>>,
    results: mpsc::Receiver<Result<()>>,
    workers: Vec<JoinHandle<()>>,
    pending: usize,
    cancelled: Arc<AtomicBool>,
}
impl Pool {
    pub fn new(threads: usize) -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(threads);
        let (complete, results) = mpsc::channel();
        let jobs = Arc::new(Mutex::new(jobs));
        let mut pool = Self {
            sender: Some(sender),
            results,
            workers: vec![],
            pending: 0,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        for index in 0..threads {
            let (jobs, complete, cancelled) = (
                Arc::clone(&jobs),
                complete.clone(),
                Arc::clone(&pool.cancelled),
            );
            pool.workers.push(
                thread::Builder::new()
                    .name(format!("cool-sort-{index}"))
                    .spawn(move || {
                        while !cancelled.load(Ordering::Relaxed) {
                            let job = jobs.lock().expect("sort queue poisoned").recv();
                            let Ok((mut rows, path)) = job else {
                                break;
                            };
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    write_run(&mut rows, &path)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("Cooler sort worker panicked"))
                                });
                            if result.is_err() {
                                cancelled.store(true, Ordering::Relaxed);
                            }
                            let _ = complete.send(result);
                        }
                    })
                    .context("cannot start Cooler sorting worker")?,
            );
        }
        Ok(pool)
    }
    fn ready(&mut self) -> Result<()> {
        while self.pending > 0 {
            match self.results.try_recv() {
                Ok(result) => {
                    self.pending -= 1;
                    result?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("Cooler sort worker disconnected")
                }
            }
        }
        Ok(())
    }
    pub fn submit(&mut self, rows: Vec<Pixel>, path: PathBuf) -> Result<()> {
        self.ready()?;
        if self.sender.as_ref().unwrap().send((rows, path)).is_err() {
            self.ready()?;
            anyhow::bail!("Cooler sort workers stopped");
        }
        self.pending += 1;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        while self.pending > 0 {
            let result = self
                .results
                .recv()
                .context("Cooler sort worker disconnected")?;
            self.pending -= 1;
            result?;
        }
        Ok(())
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
