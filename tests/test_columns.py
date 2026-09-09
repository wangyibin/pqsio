"""Small real-library columnar cross-path and ownership tests."""
from array import array
import ctypes as C
import gc
import math
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import pqsio as p
from pqsio import columns as col

ROOT = Path(__file__).parent / "output"
ROOT.mkdir(exist_ok=True)


def pairs(n=3):
    offsets, data = p.pack_strings(["", "读段", "r"][:n])
    return p.PairColumns(offsets, data, array("I", [0])*n,
                        array("Q", [2**32+1])*n, array("I", [0])*n,
                        array("Q", [2])*n, array("B", [43])*n,
                        array("B", [45])*n, array("B", [0, 1, 60][:n]))


def concat(ids=(1, 1, 2), offsets=(0, 2, 3)):
    n = len(ids)
    so, data = p.pack_strings(("", "通过", "pass")[i % 3] for i in range(n))
    return p.ConcatColumns(array("Q", offsets), array("Q", ids), array("I", [100])*n,
                           array("I", [0])*n, array("I", [50])*n,
                           array("B", [43])*n, array("I", [0])*n,
                           array("Q", [2**32])*n, array("Q", [2**32+50])*n,
                           array("B", (i % 3 for i in range(n))), array("f", [0.5])*n, so, data)


def rows(batch):
    """Test oracle only: explicitly expand columns to old record objects."""
    st = "read_id" if isinstance(batch, p.PairColumns) else "filter_reason"
    values = []
    for i in range(len(batch)):
        offsets, data = getattr(batch, st+"_offsets"), getattr(batch, st+"_bytes")
        d = {name: getattr(batch, name)[i] for name, _ in batch._schema
             if not name.endswith(("_offsets", "_bytes"))}
        d[st] = bytes(data[offsets[i]:offsets[i+1]]).decode()
        for name in d:
            if name.startswith("strand"):
                d[name] = chr(d[name])
        values.append((p.Pair if st == "read_id" else p.Alignment)(**d))
    return values


class Columns(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)
    def tearDown(self):
        self.tmp.cleanup()
    def writer(self, path, kind, chunk=2):
        return (p.PairsWriter if kind == "pairs" else p.ConcatWriter)(self.root/path, {"长染色体": 2**32+1000}, chunk_size=chunk)
    def test_cross_paths_all_fields_and_filtering(self):
        for kind, batch in (("pairs", pairs()), ("concat", concat())):
            expected = rows(batch)
            for mode in ("row", "column"):
                name = kind+mode
                with self.writer(name, kind) as w:
                    if mode == "column":
                        # Fail loudly if the implementation silently uses row encoding.
                        with patch("pqsio._encode", side_effect=AssertionError("row fallback")):
                            w.write_columns(batch)
                    elif kind == "pairs":
                        w.write_batch(expected)
                    else:
                        w.write_batch(expected, batch.read_offsets)
                for q in (0, 1, 30, 255):
                    wanted = [r for r in expected if getattr(r, "mapq", getattr(r, "mapping_quality", 0)) >= q]
                    with p.Reader(self.root/name, q) as r:
                        self.assertEqual([v for b in r.iter_batches() for v in b], wanted)
                    with p.Reader(self.root/name, q) as r:
                        with patch("pqsio._decode", side_effect=AssertionError("row fallback")):
                            result = [v for b in r.iter_columns() for v in rows(b)]
                        self.assertEqual(result, wanted)
    def test_disk_schema_matches_row_path(self):
        import polars as pl
        for kind, b in (("pairs",pairs()),("concat",concat())):
            for mode in ("row","column"):
                with self.writer(kind+mode,kind,10) as w:
                    if mode=="column": w.write_columns(b)
                    elif kind=="pairs": w.write_batch(rows(b))
                    else: w.write_batch(rows(b),b.read_offsets)
            for quality in ("q0","q1"):
                row=pl.read_parquet(self.root/(kind+"row")/quality/"0.parquet")
                column=pl.read_parquet(self.root/(kind+"column")/quality/"0.parquet")
                self.assertEqual(row.schema,column.schema)
                self.assertEqual(row.to_dicts(),column.to_dicts())
            self.assertEqual((self.root/(kind+"row")/"_metadata_counts").read_text(),
                             (self.root/(kind+"column")/"_metadata_counts").read_text())
        b=pairs(); b.pos1=array("Q",[1,2,3])
        with p.PairsWriter(self.root/"small",{"a":100}) as w: w.write_columns(b)
        self.assertEqual(pl.read_parquet(self.root/"small/q0/0.parquet").schema["pos1"],pl.UInt32)

    def test_empty(self):
        for kind, cls in (("pairs", p.PairColumns), ("concat", p.ConcatColumns)):
            batch = cls(**{n: array(t, [0] if n.endswith("_offsets") else []) for n,t in cls._schema})
            with self.writer(kind, kind) as w:
                w.write_columns(batch)
            with p.Reader(self.root/kind) as r:
                self.assertEqual(list(r.iter_columns()), [])
    def test_atomic_validation_and_cross_call_ids(self):
        with self.writer("concat", "concat", 1) as w:
            w.write_columns(concat((1,), (0,1)))
            for edit in (
                lambda b: setattr(b, "end", array("Q")),
                lambda b: setattr(b, "read_offsets", array("Q", [0,3,2])),
                lambda b: setattr(b, "read_offsets", array("Q", [0,0,2])),
                lambda b: setattr(b, "filter_reason_offsets", array("Q", [0,999,999])),
                lambda b: b.identity.__setitem__(1, math.nan),
                lambda b: b.read_idx.__setitem__(1, 1),
                lambda b: b.read_end.__setitem__(1, 101),
                lambda b: b.chrom.__setitem__(1, 1),
                lambda b: b.strand.__setitem__(1, 0),
            ):
                bad = concat((2,3), (0,1,2)); edit(bad)
                with self.assertRaises(RuntimeError): w.write_columns(bad)
            w.write_columns(concat((2,2,2,2,2), (0,5)))  # oversized complete read
            with self.assertRaises(RuntimeError): w.write_columns(concat((2,), (0,1)))
            w.write_read(rows(concat((3,), (0,1))))
            with self.assertRaises(RuntimeError): w.write_columns(concat((3,), (0,1)))
            w.write_columns(concat((4,), (0,1)))
        with p.Reader(self.root/"concat") as r:
            batches = list(r.iter_columns())
        self.assertEqual([len(b) for b in batches], [1,5,1,1])
        self.assertEqual([r.read_idx for b in batches for r in rows(b)], [1]+[2]*5+[3,4])
    def test_strings_buffers_and_pair_atomicity(self):
        with self.writer("pairs", "pairs", 1) as w:
            for edit in (
                lambda b: setattr(b, "read_id_offsets", array("Q", [0,1,0,7])),
                lambda b: setattr(b, "read_id_offsets", array("Q", [0,0,1,7])), # splits UTF-8
                lambda b: setattr(b, "read_id_offsets", array("Q", [1,1,6,7])),
                lambda b: setattr(b, "read_id_offsets", array("Q", [0])),
                lambda b: b.pos1.__setitem__(2, 0),
                lambda b: b.pos1.__setitem__(2, 2**40),
                lambda b: b.chrom1.__setitem__(2, 1),
                lambda b: b.strand1.__setitem__(2, 0),
            ):
                b = pairs(); edit(b)
                with self.assertRaises(RuntimeError): w.write_columns(b)
            b = pairs(); b.pos1 = array("I", [1,2,3])
            with self.assertRaises(ValueError): w.write_columns(b)
            b = pairs(); b.pos1 = memoryview(b.pos1)[::2]
            with self.assertRaises(ValueError): w.write_columns(b)
            b = pairs(); b.read_id_bytes = bytes(b.read_id_bytes) # readonly bulk copy
            w.write_columns(b)
        with p.Reader(self.root/"pairs") as r:
            self.assertEqual([v for b in r.iter_batches() for v in b], rows(pairs()))
    def test_python_lifetime(self):
        b = pairs()
        with self.writer("pairs", "pairs", 10) as w:
            w.write_columns(b)
            b.pos1[0] = 8 # native writer owns its copy
        with p.Reader(self.root/"pairs") as r:
            it = r.iter_columns(); b = next(it); view = memoryview(b.pos1)
            list(it)
        del b, r, it
        gc.collect()
        self.assertEqual(list(view), [2**32+1]*3)
        view[0] = 7  # Python owned and mutable
        self.assertEqual(view[0], 7)
    def test_missing_capability(self):
        class Old: pass
        with self.assertRaisesRegex(RuntimeError, "row APIs remain available"):
            col._require(Old())
    def test_actual_old_library(self):
        import os
        import subprocess
        import sys
        old = Path(__file__).resolve().parents[1]/"benchmarks/work/libpqsio-v001.so"
        if not old.is_file():
            self.skipTest("named archived v0.0.1 library unavailable")
        code = """
import pqsio as p
from pathlib import Path
import sys
for kind in ('pairs','concat'):
    path=Path(sys.argv[1])/kind
    cls=p.PairsWriter if kind=='pairs' else p.ConcatWriter
    with cls(path, {'a':100}) as w:
        if kind=='pairs': w.write_batch([p.Pair('r',0,1,0,2,'+','-',60)])
        else: w.write_read([p.Alignment(1,100,0,50,'+',0,0,50,60,0.5)])
        try: w.write_columns(None)
        except RuntimeError as e: assert 'Columnar API requires' in str(e)
        else: raise AssertionError('old library unexpectedly supports columns')
    with p.Reader(path) as r: assert len(next(r.iter_batches()))==1
    with p.Reader(path) as r:
        try: next(r.iter_columns())
        except RuntimeError as e: assert 'Columnar API requires' in str(e)
        else: raise AssertionError('old library unexpectedly supports columns')
"""
        subprocess.run([sys.executable,"-c",code,str(self.root)],
                       env=dict(os.environ, PQSIO_LIBRARY=str(old)),check=True,timeout=60)

    def test_storage_failure_poison_and_cleanup(self):
        w = self.writer("broken", "pairs", 1)
        (self.root/"broken.partial/q0").rmdir()
        with self.assertRaises(RuntimeError): w.write_columns(pairs())
        with self.assertRaisesRegex(RuntimeError, "failed"): w.write_columns(pairs())
        w.close()
        self.assertFalse((self.root/"broken.partial").exists())
        self.assertFalse((self.root/"broken").exists())
    def test_legacy_shard_local_ids(self):
        # Independent historical-layout fixture; test-only Polars dependency.
        import polars as pl
        with self.writer("legacy", "concat", 2) as w:
            w.write_columns(concat((1,1,2), (0,2,3)))
        path = self.root/"legacy"
        for f in sorted((path/"q0").glob("*.parquet")):
            df = pl.read_parquet(f)
            df.with_columns(pl.lit(0, dtype=pl.UInt64).alias("read_idx")).write_parquet(f)
        meta = (path/"_metadata").read_text().replace("'read_idx_scope': 'global'", "'read_idx_scope': 'shard'")
        self.assertIn("'read_idx_scope': 'shard'", meta)
        (path/"_metadata").write_text(meta)
        for q in (0,1,2):
            with p.Reader(path,q) as r: expected = [v for b in r.iter_batches() for v in b]
            with p.Reader(path,q) as r: actual = [v for b in r.iter_columns() for v in rows(b)]
            self.assertEqual(actual, expected)
            self.assertEqual([v.read_idx for v in actual], [1,1,2] if q==0 else [1,2] if q==1 else [2])

if __name__ == "__main__": unittest.main()
