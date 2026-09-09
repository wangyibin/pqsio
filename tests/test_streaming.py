"""Named synthetic row-group/shard fixtures; no external datasets."""
import ctypes as C
import gc
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
from test_columns import rows as column_rows

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
        frame = frame.with_columns(
            pl.Series('read_idx', ids, dtype=pl.UInt64),
            pl.Series('mapping_quality', qualities, dtype=pl.UInt8),
            pl.Series('filter_reason', [('通过', '', 'pass')[i%3] for i in range(len(ids))], dtype=pl.Utf8).cast(pl.Categorical),
            pl.Series('strand', ['+' if i%2 else '-' for i in range(len(ids))], dtype=pl.Utf8).cast(pl.Categorical),
            pl.Series('start', [2**32+i for i in range(len(ids))], dtype=pl.UInt64),
            pl.Series('end', [2**32+i+50 for i in range(len(ids))], dtype=pl.UInt64),
            pl.Series('identity', [i/16 for i in range(len(ids))], dtype=pl.Float32),
        )
        for quality in ('q0','q1'):
            for old in (path/quality).glob('*.parquet'): old.unlink()
        bounds = (0, *cuts, len(ids))
        for index, (a,b) in enumerate(zip(bounds,bounds[1:])):
            chunk=frame.slice(a,b-a)
            chunk.write_parquet(path/'q0'/f'{index}.parquet', row_group_size=2)
            chunk.filter(pl.col('mapping_quality')>=1).write_parquet(path/'q1'/f'{index}.parquet', row_group_size=3)
        if local:
            meta = path/'_metadata'
            text = meta.read_text()
            text = text.replace("'read_idx_scope': 'global'", "'read_idx_scope': 'shard'")
            meta.write_text(text)
        return path
    def batches(self, path, n=3, boundary='rows', mode=None, mapq=30):
        with p.StreamingReader(path, mapq, batch_rows=n, boundary=boundary, filter_mode=mode) as r:
            expected = list(r.iter_batches())
        with p.StreamingReader(path, mapq, batch_rows=n, boundary=boundary, filter_mode=mode) as r:
            with patch.object(p, '_decode', side_effect=AssertionError('row conversion')), \
                 patch.object(r, 'iter_batches', side_effect=AssertionError('row fallback')):
                columns = list(r.iter_columns())
        self.assertEqual([column_rows(b) for b in columns], expected)
        # Independent existing Reader decoder: q0 is the correctness oracle.
        with p.Reader(path, 0) as source:
            raw = [row for batch in source.iter_batches() for row in batch]
            if source.kind == 'pairs':
                wanted = [row for row in raw if row.mapq >= mapq]
            elif mode == 'complete_reads':
                from itertools import groupby
                wanted = []
                for _, read in groupby(raw, key=lambda row: row.read_idx):
                    read = list(read)
                    if any(row.mapping_quality >= mapq for row in read): wanted.extend(read)
            else:
                wanted = [row for row in raw if row.mapping_quality >= mapq]
        self.assertEqual([row for batch in expected for row in batch], wanted)
        for b, batch in zip(columns, expected):
            if isinstance(b, p.ConcatColumns):
                offsets = [0] + [i for i in range(1,len(batch)) if batch[i].read_idx != batch[i-1].read_idx] + [len(batch)]
                self.assertEqual(list(b.read_offsets), offsets)
        return expected
    def test_unfiltered_complete_reads_across_shards(self):
        # Includes zero-MAPQ reads, UTF-8/empty strings, long coordinates,
        # and a read spanning row groups and shards, larger than the batch.
        path = self.fixture()
        for mode in (None, 'matching_alignments'):
            for size in (1, 3, 100):
                with self.subTest(mode=mode, size=size):
                    batches = self.batches(path, n=size, boundary='complete_reads',
                                           mode=mode, mapq=0)
                    seen = set()
                    for batch in batches:
                        ids = {row.read_idx for row in batch}
                        self.assertFalse(seen & ids)
                        seen.update(ids)
                    self.assertEqual(seen, {1, 2, 3, 4})
        empty = self.fixture('empty-unfiltered', (), (), ())
        self.assertEqual(self.batches(empty, boundary='complete_reads', mapq=0), [])

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
            rows=[p.Pair(('', '读段', str(i))[i%3],0,2**32+i,0,2,'+','-',60) for i in range(7)]
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

    def test_columns_lifetime_interleaving_and_capability(self):
        path=self.fixture()
        r=p.StreamingReader(path,30,batch_rows=3,boundary='complete_reads',filter_mode='complete_reads')
        first=next(r.iter_columns())
        self.assertEqual(list(first.read_offsets),[0,2])
        self.assertEqual(len(next(r.iter_batches())),5)
        last=next(r.iter_columns())
        self.assertEqual(list(last.read_idx),[4])
        self.assertEqual(list(r.iter_columns()),[])
        self.assertEqual(list(r.iter_batches()),[])
        view=memoryview(first.start); r.close(); del first,r; gc.collect()
        self.assertEqual(list(view),[2**32,2**32+1])
        with p.StreamingReader(path,batch_rows=1) as r:
            lib=r._lib
            class RowOnly:
                def __getattr__(self,name):
                    if name=='pqsio_stream_next_columns': raise AttributeError(name)
                    return getattr(lib,name)
            r._lib=RowOnly()
            with self.assertRaisesRegex(RuntimeError,'streaming columnar capability'):
                next(r.iter_columns())
            self.assertEqual(next(r.iter_batches())[0].read_idx,1)
        with p.StreamingReader(path,batch_rows=1) as r:
            it=r.iter_columns(); next(it); r.close()
            with self.assertRaisesRegex(RuntimeError,'closed'): next(it)

    def test_columns_terminal_errors(self):
        path=self.fixture()
        r=p.StreamingReader(path,batch_rows=1)
        first=next(r.iter_columns())
        (path/'q0'/'1.parquet').write_bytes(b'corrupt')
        with self.assertRaises(RuntimeError): list(r.iter_columns())
        for method in (r.iter_columns,r.iter_batches):
            with self.assertRaisesRegex(RuntimeError,'failed'): next(method())
        r.close()
        self.assertEqual(list(first.read_idx),[1])
        path=self.fixture('bad-ids',(2,1),(60,60),())
        with p.StreamingReader(path,255,batch_rows=1) as r:
            with self.assertRaisesRegex(RuntimeError,'contiguous and increasing'): next(r.iter_columns())
        # Wrong output arguments poison live streams and clear writable outputs.
        lib=p._library(); fn=lib.pqsio_stream_next_columns
        fn.argtypes=[C.c_void_p,C.POINTER(C.c_void_p)]; fn.restype=C.c_int32
        path=self.fixture('valid')
        with p.StreamingReader(path) as r:
            self.assertEqual(fn(r._handle,None),-1)
            out=C.c_void_p(1)
            self.assertEqual(fn(r._handle,C.byref(out)),-1)
            self.assertFalse(out.value)
            with self.assertRaisesRegex(RuntimeError,'failed'): next(r.iter_batches())
        out=C.c_void_p(1)
        self.assertEqual(fn(None,C.byref(out)),-1)
        self.assertFalse(out.value)

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

    def test_q1_auto_selection_thresholds_and_partition_isolation(self):
        for kind in ('pairs','concat'):
            if kind=='pairs':
                path=self.root/'switch-pairs'
                rows=[p.Pair('dup',0,2**32+1,0,2,'+','-',q) for q in (0,1,20,30,60,60)]
                with p.PairsWriter(path,{'chr1':2**33},chunk_size=2) as w:w.write_batch(rows)
            else:
                path=self.fixture(name='switch-concat',ids=(1,1,2,2,2,4),qualities=(0,1,20,30,60,60),cuts=(2,4))
            # Compare every field against a q0 oracle before corrupting q0.
            with p.Reader(path,0) as r:raw=[x for b in r.iter_batches() for x in b]
            for threshold in (0,1,30,61,255):
                for boundary in (('rows',) if kind=='pairs' else ('rows','complete_reads')):
                    for n in (1,3,20):self.batches(path,n,boundary,mapq=threshold)
            q0=next((path/'q0').glob('*.parquet'));q0_bytes=q0.read_bytes()
            q0.write_bytes(b'corrupt unselected q0')
            for threshold in (1,30,61,255):
                expected=[row for row in raw if (row.mapq if kind=='pairs' else row.mapping_quality)>=threshold]
                for method in ('iter_batches','iter_columns'):
                    for boundary in (('rows',) if kind=='pairs' else ('rows','complete_reads')):
                        with p.StreamingReader(path,threshold,batch_rows=1,boundary=boundary) as r:
                            batches=list(getattr(r,method)())
                        actual=[row for b in batches for row in (column_rows(b) if method=='iter_columns' else b)]
                        self.assertEqual(actual,expected)
            q0.write_bytes(q0_bytes)
            # A selected q1 failure is terminal, never a restart from q0.
            q1=next((path/'q1').glob('*.parquet'));q1_bytes=q1.read_bytes()
            q1.write_bytes(b'corrupt selected q1')
            for method in ('iter_batches','iter_columns'):
                with p.StreamingReader(path,30) as r:
                    with self.assertRaises(RuntimeError):list(getattr(r,method)())
                    with self.assertRaisesRegex(RuntimeError,'failed'):next(r.iter_columns())
            with p.StreamingReader(path,0) as r:self.assertEqual([x for b in r.iter_batches() for x in b],raw)
            q1.write_bytes(q1_bytes)
            # Missing q1 is a source error, not an empty successful query.
            for f in (path/'q1').glob('*.parquet'):f.unlink()
            (path/'q1').rmdir()
            with self.assertRaises(RuntimeError):p.StreamingReader(path,30)
            with p.StreamingReader(path,0) as r:self.assertEqual([x for b in r.iter_batches() for x in b],raw)

    def test_q0_required_modes_do_not_read_q1(self):
        for local in (False,True):
            path=self.fixture(name='q0-required-'+str(local),local=local)
            for f in (path/'q1').glob('*.parquet'):f.unlink()
            (path/'q1').rmdir()
            modes=(None,'matching_alignments','complete_reads') if local else ('complete_reads',)
            for mode in modes:
                for boundary in ('rows','complete_reads'):
                    for n in (1,3,20):self.batches(path,n,boundary,mode,mapq=30)

    def test_q1_early_close_and_late_source_failure(self):
        path=self.fixture(name='q1-late',ids=(1,1,2,2,3,3),qualities=(60,60,60,60,60,60),cuts=(2,4))
        with p.StreamingReader(path,30,batch_rows=1) as r:
            first=next(r.iter_columns())
            (path/'q1'/'1.parquet').write_bytes(b'corrupt later q1 shard')
            with self.assertRaises(RuntimeError):list(r.iter_columns())
            with self.assertRaisesRegex(RuntimeError,'failed'):next(r.iter_batches())
        self.assertEqual(list(first.read_idx),[1])
        r=p.StreamingReader(path,30,batch_rows=1)
        it=r.iter_columns();next(it);r.close();r.close()
        with self.assertRaisesRegex(RuntimeError,'closed'):next(it)
        if Path('/proc/self/fd').exists():
            for fd in Path('/proc/self/fd').iterdir():
                try:target=os.readlink(fd)
                except FileNotFoundError:continue
                self.assertNotIn(str(path.resolve()),target)

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
