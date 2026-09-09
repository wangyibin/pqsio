"""Small synthetic fixtures and independent row-level oracle for subset."""
import dataclasses
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import polars as pl
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)


class Subset(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='subset-', dir=ROOT)
        self.root = Path(self.tmp.name)
        self.counter = 0

    def tearDown(self):
        self.tmp.cleanup()

    def rows(self, path, q=0):
        with p.Reader(path, min_mapq=q) as r:
            return [dataclasses.asdict(row) for batch in r.iter_batches() for row in batch]

    def pairs(self, empty=False):
        path = self.root/'pairs'
        rows = [p.Pair('重复',0,1,1,11,'+','-',0),
                p.Pair('same',0,11,1,1,'-','+',1),
                p.Pair('same',0,12,0,20,'+','+',30),
                p.Pair('boundary',0,21,1,20,'-','-',255),
                p.Pair('long',2,2**33,0,1,'+','-',60)]
        rows.insert(3, rows[2])
        with p.PairsWriter(path, {'a':100,'b':100,'long':2**34,'unused':123}, chunk_size=2) as w:
            w.write_batch([] if empty else rows)
        return path

    def concat(self, local=False, empty=False):
        path = self.root/'concat'
        with p.ConcatWriter(path, {'a':100,'b':100,'long':2**34,'unused':123}, chunk_size=100) as w:
            if not empty:
                for rid in [7,20,2**64-1]:
                    w.write_read([p.Alignment(rid,100,0,40,'+',0,10,20,30,.5,'pass"\n'),
                                  p.Alignment(rid,100,40,80,'-',1,0,10,0,.75,'low'),
                                  p.Alignment(rid,100,80,100,'+',2,2**33,2**33+20,255,1.,'')])
        if empty:
            return path
        file = next((path/'q0').glob('*.parquet'))
        frame = pl.read_parquet(file)
        file.unlink()
        # Reads span both groups and shards. Local repeated IDs remain distinct.
        if local:
            frame = frame.with_columns(pl.lit(7, dtype=pl.UInt64).alias('read_idx'))
            meta = path/'_metadata'
            meta.write_text(meta.read_text().replace("'read_idx_scope': 'global'", "'read_idx_scope': 'shard'"))
        for i,(a,b) in enumerate([(0,2),(2,5),(5,9)]):
            frame.slice(a,b-a).write_parquet(path/'q0'/f'{i}.parquet', row_group_size=1)
        return path

    def oracle(self, rows, kind, **kw):
        names = ['a','b','long','unused']
        def space(chrom, start, end):
            return ((kw.get('chroms') is None or names[chrom] in kw['chroms']) and
                    (kw.get('regions') is None or any(names[chrom] == c and start < b and end > a
                                                      for c,a,b in kw['regions'])))
        def match(r):
            if kind == 'pairs':
                points = [space(r['chrom1'],r['pos1']-1,r['pos1']), space(r['chrom2'],r['pos2']-1,r['pos2'])]
                spatial = all(points) if kw.get('pairs_mode') == 'both' else any(points)
                q, rid = r['mapq'], r['read_id']
            else:
                spatial = space(r['chrom'],r['start'],r['end'])
                q, rid = r['mapping_quality'], r['read_idx']
            return spatial and q >= (kw.get('min_mapq') or 0) and (kw.get('read_ids') is None or rid in kw['read_ids'])
        if kw.get('mode') == 'complete_reads':
            ids = {r['read_idx'] for r in rows if match(r)}
            return [r for r in rows if r['read_idx'] in ids]
        return [r for r in rows if match(r)]

    def check(self, src, **kw):
        self.counter += 1
        out = self.root/f'out{self.counter}'
        with p.Reader(src) as reader:
            kind, contigs = reader.kind, reader.contigs
        expected = self.oracle(self.rows(src),kind,**kw)
        result = p.subset(src,out,chunk_size=2,**kw).to_dict()
        self.assertEqual(self.rows(out),expected)
        key = 'mapq' if kind == 'pairs' else 'mapping_quality'
        q1 = [r for r in expected if r[key] >= 1]
        self.assertEqual(self.rows(out,1),q1)
        counts = result['counts']
        self.assertEqual(counts['q0_records'],len(expected))
        self.assertEqual(counts['q1_records'],len(q1))
        self.assertEqual(counts['q0_concats'],len({r['read_idx'] for r in expected}) if kind == 'concat' else 0)
        self.assertEqual(counts['q1_concats'],len({r['read_idx'] for r in q1}) if kind == 'concat' else 0)
        with p.Reader(out) as reader:
            self.assertEqual(reader.contigs,contigs)
        self.assertEqual(p.validate(out,'full').status,'valid')
        self.assertEqual(result['scanned_records'],len(self.rows(src)))
        self.assertTrue(result['full_scan'])
        self.assertFalse(result['index_used'])
        if kw.get('provenance',True):
            manifest = json.loads((out/'_subset.json').read_text())
            self.assertEqual(manifest['result'],result)
            if kw.get('read_ids') is not None:
                self.assertEqual([json.loads(x) for x in (out/'_subset_read_ids.jsonl').read_text().splitlines()],list(kw['read_ids']))
        else:
            self.assertIsNone(result['provenance'])
            self.assertFalse((out/'_subset.json').exists())
        return out

    def test_pairs_oracle(self):
        src = self.pairs()
        cases = [{}, *[dict(min_mapq=q) for q in [0,1,30,255]],
                 dict(chroms=['a']),dict(chroms=['a','b']),dict(chroms=[]),dict(regions=[]),dict(read_ids=[]),
                 dict(regions=[('a',10,20),('a',11,21),('b',0,1)]),
                 dict(regions=[('a',0,1)]), dict(regions=[('a',20,21)],pairs_mode='both'),
                 dict(chroms=['a'],regions=[('b',0,100)]),
                 dict(chroms=['a'],regions=[('a',10,20)],pairs_mode='both',min_mapq=30,read_ids=['same','same']),
                 dict(read_ids=['重复','same','same']),dict(regions=[('long',2**33-1,2**33)])]
        for case in cases:
            for batch in [1,4,100]:
                with self.subTest(case=case,batch=batch): self.check(src,batch_rows=batch,**case)

    def test_concat_oracle_and_complete_groups(self):
        src = self.concat()
        cases = [{},dict(min_mapq=1),dict(min_mapq=255),dict(chroms=['a'],regions=[('b',0,100)]),
                 dict(chroms=['b'],min_mapq=30),dict(regions=[]),dict(read_ids=[]),
                 dict(regions=[('a',20,30)]),dict(regions=[('a',0,10)]),
                 dict(regions=[('a',19,21),('a',10,20)],min_mapq=30),
                 dict(read_ids=[7,7,2**64-1]),dict(min_mapq=0)]
        for mode in ['matching_alignments','complete_reads']:
            for case in cases:
                for batch in [1,4,100]:
                    with self.subTest(mode=mode,case=case,batch=batch): self.check(src,mode=mode,batch_rows=batch,**case)
        out = self.check(src,mode='complete_reads',min_mapq=30,chroms=['a'],batch_rows=1)
        self.assertEqual(pl.read_parquet(out/'q0/0.parquet').height,3)

    def test_local_mapping_before_selection(self):
        src = self.concat(local=True)
        self.assertEqual([r['read_idx'] for r in self.rows(src)],[1,1,2,2,2,3,3,3,3])
        for batch in [1,3,100]:
            for mode in ['matching_alignments','complete_reads']:
                self.check(src,batch_rows=batch,mode=mode,read_ids=[2,2],min_mapq=30)

    def test_empty(self):
        for src in [self.pairs(empty=True), self.concat(empty=True)]:
            self.check(src)
            self.check(src,read_ids=[],provenance=False)

    def test_provenance_index_and_source_unchanged(self):
        src = self.pairs()
        (src/'cn.info').write_text('a\t3\n')
        def digest():
            return {str(f.relative_to(src)):hashlib.sha256(f.read_bytes()).hexdigest() for f in src.rglob('*') if f.is_file()}
        before = digest()
        out = self.check(src,regions=[('a',0,20)],provenance=False)
        expected = self.rows(out)
        self.assertEqual(before,digest())
        p.build_index(src)
        out = self.check(src,regions=[('a',0,20)])
        self.assertEqual(expected,self.rows(out))
        manifest = json.loads((out/'_subset.json').read_text())
        self.assertNotIn('cn.info',manifest['result']['omitted_sidecars'])
        self.assertIn('.pqsio-index',manifest['result']['omitted_sidecars'])
        self.assertEqual(p.read_copy_numbers(out).explicit, {'a': 3})

    def test_invalid_and_paths(self):
        src = self.pairs()
        bad = [dict(min_mapq=q) for q in [-1,256,True,1.5]] + [dict(batch_rows=0),dict(chunk_size=0),dict(batch_rows=2**32),
            dict(chroms=['missing']),dict(regions=[('missing',0,1)]),dict(regions=[('a',-1,2)]),
            dict(regions=[('a',0,101)]),dict(regions=[('a',1,1)]),dict(regions=[('a',True,2)]),
            dict(mode='complete_reads'),dict(pairs_mode='bad'),dict(read_ids=[1]),dict(read_ids=[False]),
            dict(read_ids=['a']*100001),dict(read_ids=['x'*(4*1024*1024+1)]),dict(read_ids=iter(['a']))]
        for kw in bad:
            out = self.root/'failed'
            with self.subTest(kw=str(kw)[:80]):
                with self.assertRaises((ValueError,TypeError,RuntimeError)): p.subset(src,out,**kw)
                self.assertFalse(out.exists())
                self.assertFalse(Path(str(out)+'.partial').exists())
        for out in [src,src/'nested']:
            with self.assertRaises(RuntimeError): p.subset(src,out)
        alias = self.root/'alias'; alias.symlink_to(src.resolve(),target_is_directory=True)
        with self.assertRaises(RuntimeError): p.subset(src,alias/'nested')
        out = self.root/'reserved'; staging = self.root/'reserved.partial'; staging.mkdir()
        (staging/'keep').write_text('keep')
        with self.assertRaises(RuntimeError): p.subset(src,out)
        self.assertEqual((staging/'keep').read_text(),'keep')
        dangling = self.root/'dangling'; dangling.symlink_to('absent')
        with self.assertRaises(RuntimeError): p.subset(src,dangling)
        self.assertTrue(dangling.is_symlink())
        c = self.concat()
        for kw in [dict(pairs_mode='either'),dict(read_ids=['7']),dict(read_ids=[2**64]),dict(read_ids=[-1])]:
            with self.assertRaises((ValueError,RuntimeError)): p.subset(c,out,**kw)

    def test_bad_source_cleanup_and_old_library(self):
        src = self.pairs()
        next((src/'q0').glob('*.parquet')).write_bytes(b'broken')
        out = self.root/'failed'
        with self.assertRaisesRegex(RuntimeError,'shard'): p.subset(src,out)
        self.assertFalse(out.exists()); self.assertFalse(Path(str(out)+'.partial').exists())
        with patch('pqsio._library',return_value=object()):
            with self.assertRaisesRegex(RuntimeError,'lacks subset capability'): p.subset(src,out)

    def test_invalid_unselected_fields_and_metadata(self):
        src = self.pairs()
        file = next((src/'q0').glob('*.parquet'))
        frame = pl.read_parquet(file)
        frame.with_columns(pl.lit(0,dtype=pl.UInt64).alias('pos1')).write_parquet(file)
        out = self.root/'failed'
        with self.assertRaisesRegex(RuntimeError,'positions must be 1-based'):
            p.subset(src,out,read_ids=[])
        self.assertFalse(out.exists())
        self.assertFalse(Path(str(out)+'.partial').exists())
        frame.write_parquet(file)
        meta = src/'_metadata'
        original = meta.read_text()
        for content in [original.replace("'0.1.0'", "'9.9.9'"),
                        original.rstrip()[:-1] + ", 'read_idx_scope': 'unsupported'}"]:
            meta.write_text(content)
            with self.assertRaises(RuntimeError): p.subset(src,out)
            self.assertFalse(Path(str(out)+'.partial').exists())
        meta.write_text(original)
        # q1 and declared counts are not selection data; Writer reconstructs them.
        next((src/'q1').glob('*.parquet')).write_bytes(b'broken q1')
        (src/'_metadata_counts').write_text('q0_records\t999999\n')
        self.check(src,min_mapq=1)

    def test_legacy_zero_id_and_repeated_alignments(self):
        src = self.concat()
        for file in (src/'q0').glob('*.parquet'):
            frame = pl.read_parquet(file)
            frame.with_columns(pl.when(pl.col('read_idx') == 7).then(pl.lit(0,dtype=pl.UInt64))
                               .otherwise(pl.col('read_idx')).alias('read_idx')).write_parquet(file)
        meta = src/'_metadata'
        meta.write_text(meta.read_text().replace(" 'read_idx_scope': 'global',", ''))
        self.check(src,read_ids=[0],mode='complete_reads',batch_rows=1)
        # Duplicate alignments remain distinct output rows.
        file = src/'q0/0.parquet'
        frame = pl.read_parquet(file)
        pl.concat([frame.head(1),frame]).write_parquet(file,row_group_size=1)
        self.check(src,read_ids=[0],mode='matching_alignments',batch_rows=1)

    def test_large_single_read_and_late_failure(self):
        src = self.concat()
        for file in (src/'q0').glob('*.parquet'):
            frame = pl.read_parquet(file)
            pl.concat([frame]*30).with_columns(pl.lit(7,dtype=pl.UInt64).alias('read_idx')).write_parquet(file,row_group_size=7)
        for batch in [1,64,1024]:
            out = self.check(src,batch_rows=batch,mode='complete_reads',min_mapq=255)
            self.assertEqual(pl.read_parquet(out/'q0/0.parquet').height,270)
            self.assertEqual(len(list((out/'q0').glob('*.parquet'))),1)
        src = self.pairs()
        (src/'q0/2.parquet').write_bytes(b'late corruption')
        out = self.root/'failed'
        with self.assertRaisesRegex(RuntimeError,'2.parquet'):
            p.subset(src,out,chunk_size=1,batch_rows=1)
        self.assertFalse(out.exists())
        self.assertFalse(Path(str(out)+'.partial').exists())
