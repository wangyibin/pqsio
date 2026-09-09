"""Tests against independent Polars files and CPhasing metadata templates.

Polars is a test dependency supplied by the surrounding CPhasing environment;
no Python third-party dependency is required to use pqsio itself.
"""
from pathlib import Path
import tempfile
import unittest
import polars as pl
from pqsio import Reader, Pair, Alignment, PairsWriter, ConcatWriter

ROOT=Path(__file__).parent / "output"
ROOT.mkdir(exist_ok=True)
ENV={name:getattr(pl,name) for name in ("String","Categorical","UInt8","UInt32","UInt64")}
ENV.update(pl=pl, __builtins__={})

class Compatibility(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(dir=ROOT)
        self.root=Path(self.tmp.name)
    def tearDown(self):
        self.tmp.cleanup()
    def test_schema_metadata_matches_parquet(self):
        for kind in ("pairs","concat"):
            path=self.root / kind
            if kind=="pairs":
                with PairsWriter(path,{"a":100}) as w:
                    w.write_batch([Pair("x",0,1,0,100,"+","-",0)])
            else:
                with ConcatWriter(path,{"a":100}) as w:
                    w.write_read([Alignment(1,100,0,50,"+",0,0,50,1,0.5)])
            # Same evaluation context used by CPhasing, only for our own fixture.
            meta=eval((path/"_metadata").read_text(),ENV)
            frame=pl.read_parquet(path/"q0/0.parquet")
            self.assertEqual(frame.columns, meta["columns"])
            for name, dtype in meta["schema"].items():
                self.assertEqual(frame.schema[name],dtype)
            self.assertEqual(meta["format"],kind)
    def test_existing_cphasing_python_reader(self):
        from cphasing.pqs import PQS
        for kind in ("pairs", "concat"):
            path=self.root / (kind+"_cphasing")
            if kind=="pairs":
                with PairsWriter(path,{"a":100},chunk_size=1) as w:
                    w.write_batch([Pair("x",0,1,0,100,"+","-",0),Pair("y",0,2,0,99,"-","+",30)])
            else:
                with ConcatWriter(path,{"a":100},chunk_size=1) as w:
                    w.write_read([Alignment(1,100,0,50,"+",0,0,50,0,0.5),Alignment(1,100,50,100,"-",0,50,100,30,1.0)])
            reader=PQS(str(path))
            reader.init_read()
            self.assertEqual(reader.metadata["format"],kind)
            for q, count in ((0,2),(1,1),(40,0)):
                batches=reader.read(min_mapq=q) if kind=="pairs" else reader.read_concat(min_mapq=q)
                self.assertEqual(sum(b.height for b in batches),count)

    def test_existing_cphasing_rust_writer(self):
        import subprocess
        import shutil
        binary=shutil.which("cphasing-rs")
        if not binary:
            self.skipTest("cphasing-rs executable unavailable")
        sizes=self.root/"sizes.tsv"
        sizes.write_text("a\t100\n")
        pairs=self.root/"input.pairs"
        pairs.write_text("## pairs format v1.0.0\n#chromsize: a 100\n#columns: readID chrom1 pos1 chrom2 pos2 strand1 strand2 mapq\nx\ta\t1\ta\t100\t+\t-\t0\ny\ta\t2\ta\t99\t-\t+\t30\n")
        concat=self.root/"input.concat"
        concat.write_text("1\t100\t0\t50\t+\ta\t0\t50\t0\t0.5\tpass\n1\t100\t50\t100\t-\ta\t50\t100\t30\t1.0\tpass\n")
        for kind in ("pairs","concat"):
            path=self.root/("rust_"+kind)
            args=[binary,"pairs2pqs",str(pairs),"-o",str(path),"-c","2"] if kind=="pairs" else [binary,"concat2pqs",str(concat),str(sizes),"-o",str(path),"-c","1","-t","1"]
            run=subprocess.run(args,capture_output=True,text=True,timeout=60)
            self.assertEqual(run.returncode,0,run.stderr)
            with Reader(path) as reader:
                batches=list(reader.iter_batches())
                self.assertEqual(sum(map(len,batches)),2)
            with Reader(path,min_mapq=20) as reader:
                rows=[r for batch in reader.iter_batches() for r in batch]
                self.assertEqual(len(rows),1)
                if kind=="pairs":
                    self.assertEqual(rows[0],Pair("y",0,2,0,99,"-","+",30))
                else:
                    self.assertEqual(rows[0],Alignment(1,100,50,100,"-",0,50,100,30,1.0))

    def test_local_dictionary_codes_and_unused_invalid_entries(self):
        path=self.root/"dictionary"
        self.fixture(path,"pairs")
        # Different category order in the two chromosome columns. Invalid
        # dictionary values belong only to rows removed by the MAPQ filter.
        frame=pl.DataFrame({"read_idx":["bad","one","two"],
            "chrom1":["missing","a","a"],"chrom2":["a","a","a"],
            "pos1":pl.Series([1,2,3],dtype=pl.UInt32),"pos2":pl.Series([9,8,7],dtype=pl.UInt64),
            "strand1":["invalid","-","+"],"strand2":["+","+","-"],
            "mapq":pl.Series([1,20,60],dtype=pl.UInt8)}).with_columns([
                pl.col(n).cast(pl.Categorical) for n in ["chrom1","chrom2","strand1","strand2"]])
        frame.write_parquet(path/"q0/0.parquet")
        frame.write_parquet(path/"q1/0.parquet")
        with Reader(path,min_mapq=20) as reader:
            rows=[r for b in reader.iter_batches() for r in b]
        self.assertEqual(rows,[Pair("one",0,2,0,8,"-","+",20),Pair("two",0,3,0,7,"+","-",60)])
        with Reader(path) as reader:
            with self.assertRaises(RuntimeError):
                list(reader.iter_batches())

    def test_null_integer_and_null_category_are_errors(self):
        for column in ("pos1","chrom1"):
            path=self.root/("null_"+column)
            self.fixture(path,"pairs")
            frame=pl.DataFrame({"read_idx":["r"],"chrom1":["a"],"chrom2":["a"],
                "pos1":pl.Series([1],dtype=pl.UInt32),"pos2":pl.Series([2],dtype=pl.UInt32),
                "strand1":["+"],"strand2":["-"],"mapq":pl.Series([0],dtype=pl.UInt8)})
            frame=frame.with_columns(pl.lit(None).cast(frame.schema[column]).alias(column))
            if column=="chrom1":
                frame=frame.with_columns(pl.col("chrom1").cast(pl.Categorical))
            frame.write_parquet(path/"q0/0.parquet")
            with Reader(path) as reader:
                with self.assertRaises(RuntimeError):
                    list(reader.iter_batches())

    def fixture(self, path, kind):
        (path/"q0").mkdir(parents=True)
        (path/"q1").mkdir()
        (path/"_contigsizes").write_text("a\t100\n")
        version="0.2.0" if kind=="concat" else "0.1.0"
        (path/"_metadata").write_text(repr({"is_pqs":True,"format":kind,"format-version":version,"read_idx_scope":"shard"}))
    def test_independent_concat_shards_do_not_merge_equal_ids(self):
        path=self.root/"legacy"
        self.fixture(path,"concat")
        frame=pl.DataFrame({"read_idx":pl.Series([1,1],dtype=pl.UInt64),
          "read_length":pl.Series([100,100],dtype=pl.UInt32),
          "read_start":pl.Series([0,50],dtype=pl.UInt32),"read_end":pl.Series([50,100],dtype=pl.UInt32),
          "strand":["+","-"],"chrom":["a","a"],"start":pl.Series([0,50],dtype=pl.UInt32),
          "end":pl.Series([50,100],dtype=pl.UInt32),"mapping_quality":pl.Series([0,20],dtype=pl.UInt8),
          "identity":pl.Series([0.5,1.0],dtype=pl.Float32),"filter_reason":["pass","pass"]})
        for shard in (2,10):
            frame.write_parquet(path/f"q0/{shard}.parquet")
        with Reader(path) as r:
            batches=list(r.iter_reads())
            self.assertEqual([[x.read_idx for x in b] for b in batches],[[1,1],[2,2]])
        with Reader(path,min_mapq=1) as r:
            batches=list(r.iter_reads())
            self.assertEqual([[x.read_idx for x in b] for b in batches],[[1],[2]])
    def test_independent_pairs_and_numeric_shard_order(self):
        path=self.root/"legacy_pairs"
        self.fixture(path,"pairs")
        for shard in (10,2):
            pl.DataFrame({"read_idx":[str(shard)],"chrom1":["a"],"pos1":pl.Series([1],dtype=pl.UInt32),
              "chrom2":["a"],"pos2":pl.Series([100],dtype=pl.UInt32),"strand1":["+"],"strand2":["-"],
              "mapq":pl.Series([20],dtype=pl.UInt8)}).write_parquet(path/f"q0/{shard}.parquet")
        with Reader(path) as r:
            self.assertEqual([x.read_id for b in r.iter_batches() for x in b],["2","10"])

if __name__=="__main__":
    unittest.main()
