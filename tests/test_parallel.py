"""Parallel lifecycle, shared-producer concurrency and ordered round trips."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import unittest
from pqsio import ParallelWriter, Pair, Alignment, Reader

OUTPUT = Path(__file__).resolve().parent / "output"
OUTPUT.mkdir(exist_ok=True)

def pair(i):
    return Pair(str(i), 0, i + 1, 0, i + 2, "+", "-", i % 3)

def alignment(i):
    return Alignment(i, 100, 0, 50, "+", 0, 0, 50, i % 3, 0.5)

class ParallelTests(unittest.TestCase):
    def test_shared_producer_threads_and_order(self):
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            path = Path(tmp) / "pairs"
            with ParallelWriter(path, {"chr1": 10000}, workers=4, queue_capacity=1, chunk_size=7) as w:
                with w.producer() as p:
                    p.write_batch(1, [pair(1)])
                    with ThreadPoolExecutor(4) as pool:
                        futures = [pool.submit(p.write_batch, i, [pair(i)]) for i in [0] + list(range(2, 50))]
                        for f in futures:
                            f.result(timeout=20)
            with Reader(path) as r:
                self.assertEqual([x for b in r.iter_batches() for x in b], [pair(i) for i in range(50)])
            with Reader(path, min_mapq=1) as r:
                self.assertEqual([x for b in r.iter_batches() for x in b], [pair(i) for i in range(50) if i % 3])

    def test_concat_and_producer_outlives_owner(self):
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            path = Path(tmp) / "concat"
            with ParallelWriter(path, {"chr1": 1000}, kind="concat", chunk_size=1) as w:
                p = w.producer()
                p.write_reads(0, [[alignment(0)] * 3, [alignment(1)]])
            with self.assertRaises(RuntimeError):
                p.write_reads(1, [[alignment(2)]])
            p.close()
            with Reader(path) as r:
                self.assertEqual(list(r.iter_reads()), [[alignment(0)] * 3, [alignment(1)]])

    def test_gap_invalid_batch_and_abort(self):
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            for kind in ("pairs", "concat"):
                path = Path(tmp) / kind
                w = ParallelWriter(path, {"chr1": 1000}, kind=kind)
                p = w.producer()
                if kind == "pairs":
                    p.write_batch(1, [])
                else:
                    p.write_batch(0, [alignment(0)], [0, 2])
                with self.assertRaises(RuntimeError):
                    w.finish()
                p.close()
                self.assertFalse(path.exists())
                self.assertFalse(Path(str(path) + ".partial").exists())
            w = ParallelWriter(Path(tmp) / "abort", {"chr1": 1000}, queue_capacity=1)
            p = w.producer()
            p.write_batch(1, [])
            with ThreadPoolExecutor(1) as pool:
                future = pool.submit(p.write_batch, 2, [])
                # Closing a producer with an active call defers native destruction.
                p.close()
                w.close()
                with self.assertRaises(RuntimeError):
                    future.result(timeout=10)
            self.assertFalse((Path(tmp) / "abort.partial").exists())

    def test_limits_and_options(self):
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            path = Path(tmp) / "pqs"
            for opts in ({"workers": 0}, {"queue_capacity": 0}, {"max_batch_bytes": 0}):
                with self.assertRaises(RuntimeError):
                    ParallelWriter(path, {"chr1": 1000}, **opts)
            with ParallelWriter(path, {"chr1": 1000}, max_batch_bytes=1) as w:
                with w.producer() as p:
                    with self.assertRaises(RuntimeError):
                        p.write_batch(0, [pair(0)])
                    p.write_batch(0, [])
                    with self.assertRaises(RuntimeError):
                        p.write_batch(0, [])
