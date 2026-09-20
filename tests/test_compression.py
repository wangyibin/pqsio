"""Compression round trips, level effects and compatibility of writer bindings."""
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

import pqsio as p
from test_columns import pairs, concat, rows

OUTPUT = Path(__file__).resolve().parent / "output"
OUTPUT.mkdir(exist_ok=True)


class CompressionTests(unittest.TestCase):
    def test_codecs_levels_and_writer_paths(self):
        settings = [("uncompressed", None), ("snappy", None), ("lz4", None)]
        for codec, levels in (("zstd", (None, 1, 6, 22)), ("gzip", (None, 0, 9)),
                              ("brotli", (None, 0, 11))):
            settings.extend((codec, level) for level in levels)
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            for kind, batch, cls in (("pairs", pairs(), p.PairsWriter),
                                     ("concat", concat(), p.ConcatWriter)):
                expected = rows(batch)
                for codec, level in settings:
                    for mode in ("rows", "columns", "parallel"):
                        with self.subTest(kind=kind, codec=codec, level=level, mode=mode):
                            path = Path(tmp) / f"{kind}-{codec}-{level}-{mode}"
                            opts = dict(compression=codec, compression_level=level, chunk_size=2)
                            contigs = {"chr1": 2**32 + 1000}
                            if mode == "parallel":
                                with p.ParallelWriter(path, contigs, kind=kind, **opts) as writer:
                                    with writer.producer() as producer:
                                        if kind == "pairs":
                                            producer.write_batch(0, expected)
                                        else:
                                            producer.write_batch(0, expected, batch.read_offsets)
                            else:
                                with cls(path, contigs, **opts) as writer:
                                    if mode == "columns":
                                        writer.write_columns(batch)
                                    elif kind == "pairs":
                                        writer.write_batch(expected)
                                    else:
                                        writer.write_batch(expected, batch.read_offsets)
                            for q in (0, 1, 30):
                                with p.Reader(path, q) as reader:
                                    actual = [r for b in reader.iter_batches() for r in b]
                                wanted = [r for r in expected if
                                          getattr(r, "mapq", getattr(r, "mapping_quality", 0)) >= q]
                                self.assertEqual(actual, wanted)

    def test_gzip_level_zero_is_used(self):
        # A footer records the codec, not its level. Verify level affects the
        # actual encoded output using repetitive data and gzip's extreme levels.
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            sizes = []
            records = [p.Pair("repetitive-id", 0, 1, 0, 2, "+", "-", 60)] * 2000
            for level in (0, 9):
                path = Path(tmp) / str(level)
                with p.PairsWriter(path, {"chr1": 100}, compression="gzip",
                                   compression_level=level) as writer:
                    writer.write_batch(records)
                sizes.append(sum(f.stat().st_size for q in ("q0", "q1")
                                 for f in (path / q).glob("*.parquet")))
                with p.Reader(path) as reader:
                    self.assertEqual([r for b in reader.iter_batches() for r in b], records)
            self.assertLess(sizes[1], sizes[0])

    def test_invalid_settings_create_no_output(self):
        bad = [("zstd", 0), ("zstd", 23), ("gzip", -1), ("gzip", 10),
               ("brotli", -1), ("brotli", 12), ("lz4", 1), ("snappy", 0),
               ("uncompressed", 0), ("lzo", None), ("ZSTD", None), (None, None),
               ("zstd", True), ("zstd", 1.5), ("zstd", "3"), ("zstd", 2**40)]
        with tempfile.TemporaryDirectory(dir=OUTPUT) as tmp:
            path = Path(tmp) / "invalid"
            for cls in (p.PairsWriter, p.ConcatWriter, p.ParallelWriter):
                for codec, level in bad:
                    with self.subTest(writer=cls, codec=codec, level=level):
                        with self.assertRaises((ValueError, TypeError)):
                            cls(path, {"chr1": 100}, compression=codec, compression_level=level)
                        self.assertFalse(path.exists())
                        self.assertFalse(path.with_name("invalid.partial").exists())

    def test_default_can_use_older_library(self):
        for name in ("writer_open", "parallel_open"):
            old_open = Mock(return_value=0)
            lib = SimpleNamespace(**{"pqsio_" + name: old_open})
            handle = p.C.c_void_p()
            p._open_writer(lib, name, [], "zstd", None, handle)
            old_open.assert_called_once()
            with self.assertRaisesRegex(RuntimeError, "compression selection"):
                p._open_writer(lib, name, [], "uncompressed", None, handle)
            with self.assertRaisesRegex(RuntimeError, "compression selection"):
                p._open_writer(lib, name, [], "zstd", 3, handle)


if __name__ == "__main__":
    unittest.main()
