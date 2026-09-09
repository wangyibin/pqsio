"""Named synthetic row-group/shard fixtures; no external datasets."""
import ctypes as C
import os
import shlex
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import polars as pl
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)

class Streaming(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)
    def tearDown(self):
        self.tmp.cleanup()
    def fixture(self, name='mixed', ids=(1,1,2,2,3,3,3,3,3,4), qualities=(0,60,0,0,60,0,0,0,0,60), cuts=(3,6), local=False):
        path = self.root / name
        # Writer establishes real metadata/schema. Rewrite only the named fixture.
        with p.ConcatWriter(path, {'chr1': 2**33}, chunk_size=100) as w:
            for i in range(1, 5):
                w.write_read([p.Alignment(i, 100, 0, 50, '+', 0, 2**32, 2**32+50, 60, .5, '通过')])
        frame = pl.read_parquet(next((path/'q0').glob('*.parquet')))
        frame = pl.concat([frame.slice(0,1)]*len(ids)) if ids else frame.head(0)
        frame = frame.with_columns(pl.Series('read_idx', ids, dtype=pl.UInt64), pl.Series('mapping_quality', qualities, dtype=pl.UInt8))
        for old in (path/'q0').glob('*.parquet'): old.unlink()
        bounds = (0, *cuts, len(ids))
        for index, (a,b) in enumerate(zip(bounds,bounds[1:])):
            frame.slice(a,b-a).write_parquet(path/'q0'/f'{index}.parquet', row_group_size=2)
        if local:
            meta = path/'_metadata'
            text = meta.read_text()
            text = text.replace("'read_idx_scope': 'global'", "'read_idx_scope': 'shard'")
            meta.write_text(text)
        return path
    def batches(self, path, n=3, boundary='rows', mode=None, mapq=30):
        with p.StreamingReader(path, mapq, batch_rows=n, boundary=boundary, filter_mode=mode) as r:
            return list(r.iter_batches())
    def test_combinations_and_invariance(self):
        path = self.fixture()
        for mode, expected in [('matching_alignments',[1,3,4]), ('complete_reads',[1,1,3,3,3,3,3,4])]:
            for boundary in ('rows','complete_reads'):
                oracle = None
                for n in (1,2,3,4,20):
                    with self.subTest(mode=mode,boundary=boundary,n=n):
                        batches = self.batches(path,n,boundary,mode)
                        flat = [r for b in batches for r in b]
                        self.assertEqual([r.read_idx for r in flat],expected)
                        if oracle is None: oracle=flat
                        self.assertEqual(flat,oracle)
                        self.assertTrue(all(b for b in batches))
                        if boundary=='rows':
                            self.assertTrue(all(len(b)==n for b in batches[:-1]))
                            self.assertLessEqual(len(batches[-1]),n)
                        else:
                            seen=set()
                            for batch in batches:
                                ids=set(r.read_idx for r in batch)
                                self.assertFalse(ids & seen)
                                seen |= ids
                                if len(batch)>n: self.assertEqual(len(ids),1)
        self.assertEqual([len(b) for b in self.batches(path,3,'complete_reads','complete_reads')],[2,5,1])
        self.assertEqual(len([r for b in self.batches(path,2,mode='complete_reads',mapq=0) for r in b]),10)
    def test_local_ids_before_filter_and_shard_separation(self):
        path=self.fixture(ids=(1,1,2,1,1,2), qualities=(0,0,60,0,60,60),cuts=(3,),local=True)
        for n in (1,3,20):
            for mode,ids in [('matching_alignments',[2,3,4]),('complete_reads',[2,3,3,4])]:
                for boundary in ('rows','complete_reads'):
                    self.assertEqual([r.read_idx for b in self.batches(path,n,boundary,mode) for r in b],ids)
    def test_empty_shards_empty_data_all_filtered(self):
        for name,ids,qs,cuts in [('empty',(),(),()),('filtered',(1,1),(0,0),(0,1)),('empties',(1,1),(60,60),(0,1,1))]:
            path=self.fixture(name,ids,qs,cuts)
            for mode in ('matching_alignments','complete_reads'):
                for boundary in ('rows','complete_reads'):
                    flat=[r for b in self.batches(path,2,boundary,mode) for r in b]
                    self.assertEqual(len(flat),2 if name=='empties' else 0)
    def test_pairs_split_pack_long_coordinates_and_legacy(self):
        for chunk in (2,20):
            path=self.root/f'pairs{chunk}'
            rows=[p.Pair(str(i),0,2**32+i,0,2,'+','-',60) for i in range(7)]
            with p.PairsWriter(path,{'chr1':2**33},chunk_size=chunk) as w: w.write_batch(rows)
            for n in (1,3,7,20):
                batches=self.batches(path,n)
                self.assertEqual([r for b in batches for r in b],rows)
                self.assertTrue(all(len(b)==n for b in batches[:-1]))
            for boundary,mode in [('complete_reads',None),('rows','matching_alignments'),('rows','complete_reads')]:
                with self.assertRaisesRegex(RuntimeError,'pairs requires'):
                    self.batches(path,boundary=boundary,mode=mode)
            with p.Reader(path) as r:
                self.assertEqual([len(b) for b in r.iter_batches()], [2,2,2,1] if chunk==2 else [7])
    def test_options_capability_close_and_failure(self):
        path=self.fixture()
        for n in (0,-1,2**32,2**64,1.5):
            with self.assertRaises(ValueError): p.StreamingReader(path,batch_rows=n)
        with self.assertRaises(ValueError): p.StreamingReader(path,boundary='bad')
        with patch.object(p,'_library',return_value=object()):
            with self.assertRaisesRegex(RuntimeError,'lacks streaming capability'): p.StreamingReader(path)
        r=p.StreamingReader(path,batch_rows=1)
        it=r.iter_batches(); next(it); r.close(); r.close()
        with self.assertRaisesRegex(RuntimeError,'closed'): next(it)
        (path/'q0'/'1.parquet').write_bytes(b'corrupt')
        r=p.StreamingReader(path,batch_rows=1)
        with self.assertRaises(RuntimeError): list(r.iter_batches())
        with self.assertRaisesRegex(RuntimeError,'failed'): next(r.iter_batches())
        r.close()
        invalid=self.fixture('unordered',(1,2,1),(60,60,60),())
        with self.assertRaisesRegex(RuntimeError,'contiguous and increasing'): self.batches(invalid)
    def test_old_library_capability(self):
        old=Path(__file__).resolve().parents[1]/'benchmarks/work/libpqsio-v001.so'
        if not old.is_file(): self.skipTest('named archived v0.0.1 library unavailable')
        path=self.fixture()
        code="""
import pqsio as p, sys
with p.Reader(sys.argv[1]) as r:
    assert next(r.iter_batches())
try: p.StreamingReader(sys.argv[1])
except RuntimeError as e: assert 'lacks streaming capability' in str(e)
else: raise AssertionError('missing capability error')
"""
        subprocess.run([sys.executable,'-c',code,str(path)],
                       env=dict(os.environ,PQSIO_LIBRARY=str(old)),check=True)

    def test_close_releases_shard_descriptors(self):
        path=self.fixture()
        r=p.StreamingReader(path,batch_rows=1)
        it=r.iter_batches(); next(it)
        def handles():
            found=[]
            for fd in Path('/proc/self/fd').iterdir():
                try:
                    target=os.readlink(fd)
                    if str(path) in target: found.append(target)
                except FileNotFoundError: pass
            return found
        self.assertTrue(handles())
        r.close()
        self.assertFalse(handles())
        it.close()

    def test_native_c_cpp(self):
        path=self.fixture()
        project=Path(__file__).resolve().parents[1]
        library=Path(os.environ['PQSIO_LIBRARY']).resolve().parent
        for ext,env,default,std in [('c','CC','cc','c11'),('cpp','CXX','c++','c++17')]:
            exe=self.root/('native_'+ext)
            subprocess.run(shlex.split(os.environ.get(env,default))+[
                '-std='+std,'-Wall','-Wextra','-Werror','-I',str(project/'include'),
                str(project/'tests'/('streaming.'+ext)), '-L',str(library),'-lpqsio',
                '-Wl,-rpath,'+str(library),'-o',str(exe)],check=True,capture_output=True,text=True)
            subprocess.run([str(exe),str(path)],check=True)

    def test_c_invalid_and_callback_failure(self):
        path=self.fixture(); lib=p._library(); handle=C.c_void_p()
        for n,b,f in [(0,0,0),(2**32,0,0),(1,2,0),(1,0,3)]:
            self.assertEqual(lib.pqsio_stream_open(str(path).encode(),0,n,b,f,C.byref(handle)),-1)
            self.assertFalse(handle.value)
        self.assertEqual(lib.pqsio_stream_open(str(path).encode(),0,1,0,0,C.byref(handle)),0)
        pairs=p._PairsCB(lambda *args: 0); bad=p._ConcatCB(lambda *args: -1)
        self.assertEqual(lib.pqsio_stream_next(handle,pairs,bad,None),-1)
        self.assertEqual(lib.pqsio_stream_next(handle,pairs,bad,None),-1)
        self.assertIn(b'failed',lib.pqsio_last_error())
        lib.pqsio_stream_destroy(handle)

if __name__=='__main__': unittest.main()
