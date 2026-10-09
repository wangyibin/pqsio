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
    def test_bulk_concat_boundaries_and_atomic_rejection(self):
        a=lambda i,q: Alignment(i,100,0,50,"+",0,0,50,q,0.5)
        with ConcatWriter(self.path, {"a":100}, chunk_size=2) as w:
            w.write_reads([])
            for offsets in ([], [1,2], [0,0,2], [0,3,2], [0,1]):
                with self.assertRaises(RuntimeError):
                    w.write_batch([a(1,0),a(2,1)],offsets)
            with self.assertRaises(RuntimeError):
                w.write_reads([[a(1,0)],[a(1,1)]])
            with self.assertRaises(RuntimeError):
                w.write_reads([[],[a(1,0)]])
            with self.assertRaises(ValueError):
                w.write_batch([a(1,0)],[-1,1])
            w.write_reads([[a(1,0),a(1,1),a(1,30)],[a(2,0)]])
            w.write_read([a(3,1)])
        with Reader(self.path) as r:
            self.assertEqual([[x.read_idx for x in b] for b in r.iter_batches()],[[1,1,1],[2,3]])
        with ConcatReader(self.path,min_mapq=1) as r:
            self.assertEqual([[x.read_idx for x in b] for b in r.iter_reads()],[[1,1],[3]])

    def test_bulk_null_offsets_and_closed_handle(self):
        import pqsio
        with ConcatWriter(self.path,{"a":100}) as w:
            lib=pqsio._library()
            self.assertEqual(lib.pqsio_write_reads(w._handle,None,0,None,1),-1)
            self.assertIn(b"null array",lib.pqsio_last_error())
        with self.assertRaises(RuntimeError):
            w.write_reads([])

    def test_fast_encoding_keeps_validation(self):
        import pqsio
        from dataclasses import replace
        pair=Pair("r",0,1,0,2,"+","-",1)
        for name, value in (("chrom1",-1),("chrom2",2**32),("pos1",2**64),
                            ("pos2",1.5),("mapq",256),("strand1","++"),("read_id","a\0b")):
            with self.subTest(field=name), self.assertRaises(ValueError):
                pqsio._encode(replace(pair,**{name:value}),pqsio._Pair)
        row=Alignment(1,100,0,50,"+",0,0,50,1,0.5)
        for name, value in (("read_idx",-1),("read_length",2**32),("read_start",-1),
                            ("read_end",2**32),("strand","?"),("chrom",2**32),
                            ("start",-1),("end",2**64),("mapping_quality",256),
                            ("filter_reason","a\0b")):
            with self.subTest(field=name), self.assertRaises(ValueError):
                pqsio._encode(replace(row,**{name:value}),pqsio._Alignment)

    def test_null_ffi_handle_reports_error(self):
        import pqsio
        lib=pqsio._library()
        self.assertEqual(lib.pqsio_writer_finish(None), -1)
        self.assertIn(b"null writer",lib.pqsio_last_error())

class LibraryDiscoveryTest(unittest.TestCase):
    def test_environment_prefix_without_library_variable(self):
        import pqsio
        from unittest.mock import patch
        library = Path(os.environ["PQSIO_LIBRARY"]).resolve()
        with tempfile.TemporaryDirectory(dir=ROOT) as directory:
            prefix = Path(directory)
            (prefix / "lib").mkdir()
            (prefix / "lib/libpqsio.so").symlink_to(library)
            with patch.object(pqsio, "_lib", None), \
                    patch.object(pqsio.sys, "prefix", str(prefix)), \
                    patch.dict(os.environ, {"PQSIO_LIBRARY": ""}), \
                    patch("ctypes.util.find_library", side_effect=AssertionError("system lookup used")):
                self.assertEqual(pqsio._library().pqsio_abi_version(), 1)

    def test_explicit_library_override_is_preserved(self):
        import pqsio
        from unittest.mock import patch
        with patch.object(pqsio, "_lib", None), \
                patch.dict(os.environ, {"PQSIO_LIBRARY": "/missing/pqsio-library.so"}):
            with self.assertRaises(OSError):
                pqsio._library()


if __name__ == "__main__":
    unittest.main()
