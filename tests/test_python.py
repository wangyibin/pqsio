import ctypes as C
import os
from pathlib import Path
import tempfile
import unittest
from pqsio import Pair, Alignment, PairsWriter, ConcatWriter, Reader, ConcatReader

ROOT = Path(__file__).parent / "output"
ROOT.mkdir(exist_ok=True)

class StorageTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.path = Path(self.tmp.name) / "data.pqs"
    def tearDown(self):
        self.tmp.cleanup()
    def test_pairs_long_coordinate_unicode_and_filter(self):
        rows = [Pair("读段", 0, 1, 1, 2**32+1, "+", "-", q) for q in (0, 1, 60)]
        with PairsWriter(self.path, [("a", 100), ("b", 2**32+100)], chunk_size=2) as w:
            w.write_batch(rows)
        with Reader(self.path) as r:
            self.assertEqual([x for b in r.iter_batches() for x in b], rows)
        with Reader(self.path, min_mapq=20) as r:
            self.assertEqual([x for b in r.iter_batches() for x in b], rows[2:])
        with self.assertRaises(RuntimeError):
            PairsWriter(self.path, [("a", 100)])
    def test_concat_complete_read(self):
        rows = [Alignment(1, 100, 0, 50, "+", 0, 0, 50, q, 0.5) for q in (0, 1, 60)]
        with ConcatWriter(self.path, {"a":100}, chunk_size=2) as w:
            w.write_read(rows)
            w.write_read([Alignment(2, 100, 50, 100, "-", 0, 50, 100, 0, 1.0)])
        with ConcatReader(self.path) as r:
            batches=list(r.iter_batches())
            self.assertEqual([len(b) for b in batches], [3,1])
            self.assertEqual(batches[0], rows)
        with ConcatReader(self.path, min_mapq=1) as r:
            self.assertEqual(list(r.iter_reads()), [rows[1:]])
        counts=dict(line.split() for line in (self.path / "_metadata_counts").read_text().splitlines())
        self.assertEqual(counts, dict(q0_records="4",q1_records="2",q0_concats="2",q1_concats="1"))
    def test_abort_and_integer_range(self):
        with self.assertRaises(ValueError):
            with PairsWriter(self.path, {"a":100}) as w:
                w.write_batch([Pair("r",0,1,0,2,"+","-",256)])
        self.assertFalse(self.path.exists())
        self.assertFalse(Path(str(self.path)+".partial").exists())
    def test_empty_dataset(self):
        with PairsWriter(self.path, {"a":100}):
            pass
        with Reader(self.path) as r:
            self.assertEqual(list(r.iter_batches()), [])
    def test_closed_handles(self):
        w=PairsWriter(self.path, {"a":100})
        w.finish()
        with self.assertRaises(RuntimeError):
            w.write_batch([])
        r=Reader(self.path)
        r.close()
        with self.assertRaises(RuntimeError):
            list(r.iter_batches())
    def test_null_ffi_handle_reports_error(self):
        import pqsio
        lib=pqsio._library()
        self.assertEqual(lib.pqsio_writer_finish(None), -1)
        self.assertIn(b"null writer",lib.pqsio_last_error())

if __name__ == "__main__":
    unittest.main()
