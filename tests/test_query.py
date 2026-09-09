"""Small named synthetic region-index fixtures; no downloads or test dependencies."""
import ctypes as C
import gc
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import polars as pl
import pqsio as p
from test_columns import rows as column_rows
import test_streaming

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)


class Queries(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='query-', dir=ROOT)
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def pairs(self, name='pairs', records=None, group=2, chunk=100):
        if records is None:
            records = [p.Pair('duplicate',0,1,1,101,'+','-',60),
                       p.Pair('duplicate',0,1,1,101,'+','-',60),
                       p.Pair('boundary',0,10,0,11,'-','+',30),
                       p.Pair('low',0,11,1,12,'+','+',0),
                       p.Pair('long',0,2**32+1,1,2**32+2,'+','-',60),
                       p.Pair('last',1,500,1,900,'-','-',20)]
        path = self.root / name
        with p.PairsWriter(path, {'chr1':2**33,'chr2':2**33},chunk_size=chunk) as w:
            w.write_batch(records)
        for shard in (path/'q0').glob('*.parquet'):
            frame = pl.read_parquet(shard)
            frame.write_parquet(shard,row_group_size=group,statistics=False)
        return path

    def concat(self, **kwargs):
        helper = test_streaming.Streaming()
        helper.root = self.root
        path = helper.fixture(**kwargs)
        self.sync_q1(path)
        return path

    def sync_q1(self, path):
        # Rewritten synthetic q0 fixtures must retain a valid q1 view.
        for shard in (path/'q1').glob('*.parquet'):
            shard.unlink()
        for shard in sorted((path/'q0').glob('*.parquet')):
            frame = pl.read_parquet(shard)
            quality = 'mapq' if 'mapq' in frame.columns else 'mapping_quality'
            frame.filter(pl.col(quality) >= 1).write_parquet(path/'q1'/shard.name, row_group_size=3)

    def build_both(self, path):
        p.build_index(path)
        p.build_index(path, quality='q1')

    def collect(self, path, regions, index='off', **options):
        with p.QueryReader(path, regions, index=index, **options) as r:
            columns = list(r.iter_columns())
            records = [row for b in columns for row in column_rows(b)]
            stats = r.stats
        self.assertTrue(stats['complete'])
        self.assertEqual(stats['returned_rows'],len(records))
        self.assertEqual(stats,r.stats)
        self.assertEqual(stats['summary_cache_bytes'],0)
        return records, stats

    def oracle(self, path, regions, min_mapq=0, pairs_mode='either', filter_mode=None, **_):
        with p.Reader(path,0) as r:
            raw = [row for b in r.iter_batches() for row in b]
            names = [name for name,_ in r.contigs]
        def overlap(chrom,start,end):
            return any(names[chrom]==name and start < b and end > a for name,a,b in regions)
        def match(row):
            if isinstance(row,p.Pair):
                a=overlap(row.chrom1,row.pos1-1,row.pos1)
                b=overlap(row.chrom2,row.pos2-1,row.pos2)
                return row.mapq>=min_mapq and (a and b if pairs_mode=='both' else a or b)
            return row.mapping_quality>=min_mapq and overlap(row.chrom,row.start,row.end)
        if filter_mode=='complete_reads':
            from itertools import groupby
            output=[]
            for _,read in groupby(raw,key=lambda r:r.read_idx):
                read=list(read)
                if any(map(match,read)): output.extend(read)
            return output
        return list(filter(match,raw))

    def compare(self,path,regions,**options):
        expected=self.oracle(path,regions,**options)
        off,_=self.collect(path,regions,'off',**options)
        indexed,stats=self.collect(path,regions,'require',**options)
        self.assertEqual(off,expected)
        self.assertEqual(indexed,off)
        return indexed,stats

    def generation(self,path,quality='q0'):
        root=path/'.pqsio-index'
        if quality=='q1':root=root/'q1'
        return root/(root/'CURRENT').read_text()

    def test_zero_mapq_complete_boundary_still_filters_regions(self):
        path = self.concat(name='zero-mapq-query')
        self.build_both(path)
        for regions in ([], [('chr1', 2**32+53, 2**32+54)]):
            rows, _ = self.compare(path, regions, min_mapq=0,
                                   boundary='complete_reads',
                                   filter_mode='matching_alignments', batch_rows=1)
            self.assertLess(len(rows), 10)

    def test_pairs_combinations_boundaries_duplicates(self):
        path=self.pairs(); self.build_both(path)
        region_sets=[[], [('chr1',0,1)], [('chr1',9,10)], [('chr1',10,11)],
                     [('chr1',0,11),('chr1',0,1),('chr2',100,101)],
                     [('chr1',2**32,2**32+1)], [('chr1',0,2**33),('chr2',0,2**33)],
                     [('chr2',1000,1001)]]
        for regions in region_sets:
            for mode in ('either','both'):
                for mapq in (0,30,60,61):
                    for n in (1,3,20):
                        with self.subTest(regions=regions,mode=mode,mapq=mapq,n=n):
                            self.compare(path,regions,pairs_mode=mode,min_mapq=mapq,batch_rows=n)
        found,stats=self.compare(path,[('chr1',0,1),('chr1',0,2)])
        self.assertEqual(len(found),2)
        self.assertGreater(stats['skipped_row_groups'],0)
        self.assertLess(stats['decoded_row_groups'],stats['total_row_groups'])
        self.assertEqual(found[0].pos1,1)

    def test_u64_upper_coordinate_boundary(self):
        maximum=2**64-1
        for kind in ('pairs','concat'):
            path=self.root/('u64-'+kind)
            if kind=='pairs':
                with p.PairsWriter(path,{'chr1':maximum}) as writer:
                    writer.write_batch([p.Pair('max',0,maximum,0,maximum,'+','-',60)])
            else:
                with p.ConcatWriter(path,{'chr1':maximum}) as writer:
                    writer.write_read([p.Alignment(1,100,0,50,'+',0,maximum-50,maximum,60,.5)])
            self.build_both(path)
            self.assertEqual(len(self.compare(path,[('chr1',maximum-1,maximum)])[0]),1)
            self.assertEqual(self.compare(path,[('chr1',0,1)])[0],[])

    def test_summary_false_positive_both_and_mapq(self):
        records=[p.Pair('a',0,10,1,90,'+','-',0),p.Pair('b',1,90,0,10,'+','-',60),p.Pair('c',0,90,1,90,'+','-',60)]
        path=self.pairs(records=records,group=3);self.build_both(path)
        result,stats=self.compare(path,[('chr1',9,10)],pairs_mode='both')
        self.assertEqual(result,[])
        self.assertEqual(stats['candidate_row_groups'],1)
        result,stats=self.compare(path,[('chr1',9,10)],pairs_mode='both',min_mapq=60)
        self.assertEqual(result,[])
        self.assertEqual(stats['decoded_row_groups'],0)

    def test_concat_modes_cross_group_shard_and_local_ids(self):
        for local in (False,True):
            path=self.concat(name='concat'+str(local),local=local)
            self.build_both(path)
            for regions in ([('chr1',2**32+2,2**32+3)], [('chr1',2**32+53,2**32+54)],
                            [('chr1',2**32-1,2**32)],[],[('chr1',0,2**33)]):
                for mode in ('matching_alignments','complete_reads'):
                    for boundary in ('rows','complete_reads'):
                        for n in (1,3,20):
                            with self.subTest(local=local,regions=regions,mode=mode,boundary=boundary,n=n):
                                result,stats=self.compare(path,regions,min_mapq=30,filter_mode=mode,boundary=boundary,batch_rows=n)
                                self.assertEqual(stats['index_used'],not local and mode!='complete_reads')
                                if not stats['index_used']: self.assertIn('sequential',stats['fallback_reason'])
            # Column and row output share the exact cursor/predicate.
            options=dict(regions=[('chr1',2**32+53,2**32+54)],filter_mode='complete_reads',batch_rows=2)
            with p.QueryReader(path,**options) as r: rows=[x for b in r.iter_batches() for x in b]
            self.assertEqual(rows,self.collect(path,**options)[0])

    def test_local_mapping_skipped_prefix_is_never_renumbered(self):
        path=self.concat(name='local',ids=(1,1,2,1,1,2),qualities=(0,0,60,0,60,60),cuts=(3,),local=True)
        self.build_both(path)
        records,stats=self.compare(path,[('chr1',2**32+54,2**32+55)],min_mapq=30)
        self.assertEqual([r.read_idx for r in records],[4])
        self.assertFalse(stats['index_used'])

    def test_empty_shards_dataset_and_no_hits(self):
        for kind in ('pairs','concat'):
            path=self.pairs(name='empty-pairs',records=[]) if kind=='pairs' else self.concat(name='empty-concat',ids=(),qualities=(),cuts=())
            self.build_both(path)
            self.assertEqual(self.compare(path,[('chr1',0,1)])[0],[])
        path=self.concat(name='empty-between',ids=(1,1,2),qualities=(60,60,60),cuts=(0,2,2))
        self.build_both(path)
        self.compare(path,[('chr1',0,2**33)])
        self.compare(path,[('chr1',0,1)])

    def test_invalid_regions_options(self):
        path=self.pairs()
        for regions in ([('missing',0,1)],[('chr1',1,1)],[('chr1',2,1)],[('chr1',0,2**33+1)]):
            with self.assertRaises(RuntimeError):p.QueryReader(path,regions)
        for regions in ([('chr1',-1,1)],[('chr1',0,2**64)],[('chr1',False,1)]):
            with self.assertRaises(ValueError):p.QueryReader(path,regions)
        for options in (dict(index='bad'),dict(pairs_mode='bad'),dict(batch_rows=0),dict(boundary='bad')):
            with self.assertRaises(ValueError):p.QueryReader(path,[],**options)

    def test_missing_stale_corrupt_versions_and_require(self):
        path=self.pairs();regions=[('chr1',0,11)]
        expected=self.collect(path,regions)[0]
        def unavailable():
            rows,stats=self.collect(path,regions,'auto');self.assertEqual(rows,self.oracle(path,regions))
            self.assertFalse(stats['index_used']);self.assertTrue(stats['fallback_reason'])
            with self.assertRaisesRegex(RuntimeError,'required index unavailable'):p.QueryReader(path,regions,index='require')
        unavailable();self.build_both(path)
        with self.assertRaisesRegex(RuntimeError,'rebuild'):self.build_both(path)
        manifest=self.generation(path)/'manifest.json';original=manifest.read_text()
        for change in ('unknown','corrupt','incomplete'):
            if change=='corrupt':manifest.write_text('{broken')
            else:
                m=json.loads(original);m['version' if change=='unknown' else 'complete']='unknown' if change=='unknown' else False
                manifest.write_text(json.dumps(m))
            unavailable();manifest.write_text(original)
        part=self.generation(path)/'0.rg';raw=part.read_bytes();part.write_bytes(raw[:-1]);unavailable();part.write_bytes(raw)
        for name in ('_metadata','_contigsizes'):
            file=path/name;original_source=file.read_bytes();file.write_bytes(original_source+b'\n' if name=='_metadata' else original_source.replace(b'\t',b'  '));unavailable();file.write_bytes(original_source)
        shard=next((path/'q0').glob('*.parquet'));stat=shard.stat()
        os.utime(shard,ns=(stat.st_atime_ns,stat.st_mtime_ns+1000000));unavailable()
        p.build_index(path,rebuild=True)
        self.assertTrue(self.collect(path,regions,'require')[1]['index_used'])
        # Inventory changes invalidate; q1 is intentionally irrelevant.
        extra=path/'q0'/'extra.parquet';extra.write_bytes(shard.read_bytes());unavailable();extra.unlink()
        self.assertTrue(self.collect(path,regions,'require')[1]['index_used'])

    def test_footer_fingerprint_and_source_errors_not_swallowed(self):
        path=self.pairs();self.build_both(path)
        shard=next((path/'q0').glob('*.parquet'));original=shard.read_bytes();stat=shard.stat()
        # Rewrite valid footer with same length and restore mtime; fingerprint still detects it.
        raw=bytearray(original);at=raw.rfind(b'Polars')
        self.assertGreaterEqual(at,0)
        if at>=0:
            raw[at:at+6]=b'polars';shard.write_bytes(raw);os.utime(shard,ns=(stat.st_atime_ns,stat.st_mtime_ns))
            _,stats=self.collect(path,[('chr1',0,11)],'auto');self.assertFalse(stats['index_used'])
        shard.write_bytes(b'broken')
        with self.assertRaises(RuntimeError):p.QueryReader(path,[('chr1',0,11)],index='auto')

    def test_failed_rebuild_keeps_published_generation_and_source_readonly(self):
        path=self.pairs();self.build_both(path)
        current=(path/'.pqsio-index'/'CURRENT').read_bytes()
        shard=next((path/'q0').glob('*.parquet'));original=shard.read_bytes();stat=shard.stat()
        frame=pl.read_parquet(shard).with_columns(pl.lit(0,dtype=pl.UInt64).alias('pos1'))
        frame.write_parquet(shard,row_group_size=2)
        with self.assertRaisesRegex(RuntimeError,'position'):p.build_index(path,rebuild=True)
        self.assertEqual((path/'.pqsio-index'/'CURRENT').read_bytes(),current)
        self.assertFalse(list((path/'.pqsio-index').glob('*.partial')))
        shard.write_bytes(original);os.utime(shard,ns=(stat.st_atime_ns,stat.st_mtime_ns))
        sources=[path/'_metadata',path/'_contigsizes',path/'_metadata_counts',*sorted((path/'q0').glob('*.parquet')),*sorted((path/'q1').glob('*.parquet'))]
        before={f:(hashlib.sha256(f.read_bytes()).hexdigest(),f.stat().st_mtime_ns) for f in sources}
        for f in sources:f.chmod(0o444)
        try:
            p.build_index(path,rebuild=True)
            self.compare(path,[('chr1',0,11)])
            self.assertEqual(before,{f:(hashlib.sha256(f.read_bytes()).hexdigest(),f.stat().st_mtime_ns) for f in sources})
        finally:
            for f in sources:f.chmod(0o644)

    def test_partial_stats_resource_release_late_failure_no_restart(self):
        path=self.pairs(group=2,chunk=2);self.build_both(path)
        reader=p.QueryReader(path,[('chr1',0,2**33),('chr2',0,2**33)],index='require',batch_rows=1)
        self.assertFalse(reader.stats['complete'])
        iterator=reader.iter_columns();batch=next(iterator)
        self.assertEqual(reader.stats['returned_rows'],1)
        self.assertFalse(reader.stats['complete'])
        # Corrupt a not-yet-opened partition after preflight: terminal, no fallback.
        (self.generation(path)/'1.rg').write_bytes(b'broken')
        with self.assertRaises(RuntimeError):list(iterator)
        self.assertFalse(reader.stats['complete'])
        with self.assertRaisesRegex(RuntimeError,'failed'):next(reader.iter_columns())
        reader.close();reader.close();self.assertEqual(len(batch),1)
        with self.assertRaisesRegex(RuntimeError,'closed'):next(reader.iter_columns())
        p.build_index(path,rebuild=True)
        for _ in range(4):
            r=p.QueryReader(path,[('chr1',0,11)]);next(r.iter_columns());r.close()
        gc.collect()
        if Path('/proc/self/fd').exists():
            for fd in Path('/proc/self/fd').iterdir():
                try:target=os.readlink(fd)
                except FileNotFoundError:continue
                self.assertNotIn(str(path.resolve()),target)

    def test_fixed_seed_randomized_mixed_contigs(self):
        rng=random.Random(71291)
        records=[p.Pair(str(i%9),rng.randrange(2),rng.randrange(1,1001),rng.randrange(2),rng.randrange(1,1001),'+','-',rng.choice([0,20,30,60])) for i in range(240)]
        path=self.pairs(name='random-mixed',records=records,group=13,chunk=67);self.build_both(path)
        for _ in range(35):
            regions=[]
            for __ in range(rng.randrange(5)):
                start=rng.randrange(999);regions.append((rng.choice(['chr1','chr2']),start,min(1000,start+rng.randrange(1,70))))
            self.compare(path,regions,pairs_mode=rng.choice(['either','both']),min_mapq=rng.choice([0,30,60]),batch_rows=rng.choice([1,17,80]))

    def test_invalid_summary_inputs_never_publish(self):
        cases = [('zero', 'pos1', 0, pl.UInt64),
                 ('too-long', 'pos2', 2**33+1, pl.UInt64),
                 ('unknown-contig', 'chrom1', 'missing', pl.Utf8),
                 ('null-mapq', 'mapq', None, pl.UInt8)]
        for name,column,value,dtype in cases:
            path=self.pairs(name=name)
            shard=next((path/'q0').glob('*.parquet'))
            frame=pl.read_parquet(shard).with_columns(pl.lit(value,dtype=dtype).alias(column))
            frame.write_parquet(shard,row_group_size=2)
            with self.assertRaises(RuntimeError):self.build_both(path)
            self.assertFalse((path/'.pqsio-index'/'CURRENT').exists())
            self.assertFalse(list((path/'.pqsio-index').glob('*.partial')))
        path=self.concat(name='reversed-interval')
        shard=next((path/'q0').glob('*.parquet'))
        pl.read_parquet(shard).with_columns(pl.lit(1,dtype=pl.UInt64).alias('end')).write_parquet(shard)
        with self.assertRaisesRegex(RuntimeError,'coordinates'):self.build_both(path)

    def test_fixed_seed_random_concat_reads(self):
        rng=random.Random(919)
        ids=[]
        for read in range(1,65):ids.extend([read]*rng.randrange(1,5))
        path=self.concat(name='random-concat',ids=ids,qualities=[rng.choice([0,30,60]) for _ in ids],cuts=(23,79))
        for shard in sorted((path/'q0').glob('*.parquet')):
            frame=pl.read_parquet(shard)
            starts=[rng.randrange(1000) for _ in range(len(frame))]
            frame=frame.with_columns(pl.Series('start',starts,dtype=pl.UInt64),
                                     pl.Series('end',[a+rng.randrange(1,100) for a in starts],dtype=pl.UInt64))
            frame.write_parquet(shard,row_group_size=7)
        self.sync_q1(path)
        self.build_both(path)
        for _ in range(20):
            a=rng.randrange(1000)
            self.compare(path,[('chr1',a,a+30),('chr1',a+10,a+80)],
                         filter_mode=rng.choice(['matching_alignments','complete_reads']),
                         min_mapq=rng.choice([0,30,60]),batch_rows=rng.choice([1,13,99]),
                         boundary=rng.choice(['rows','complete_reads']))

    def test_quality_selection_and_partition_counters(self):
        for kind in ('pairs', 'concat'):
            path = self.pairs(name='switch-pairs',group=1) if kind=='pairs' else self.concat(name='switch-concat')
            self.sync_q1(path)
            self.build_both(path)
            regions = [('chr1',0,2**33)]
            observed = p.inspect(path).to_dict()['observed']['shards']
            for index in ('auto','off','require'):
                for mapq in (0,1,30,61):
                    for n in (1,4,50):
                        for boundary in (('rows',) if kind=='pairs' else ('rows','complete_reads')):
                            with self.subTest(kind=kind,index=index,mapq=mapq,n=n,boundary=boundary):
                                rows, stats = self.collect(path,regions,index,min_mapq=mapq,batch_rows=n,boundary=boundary)
                                self.assertEqual(rows,self.oracle(path,regions,min_mapq=mapq))
                                source = 'q1' if mapq>0 else 'q0'
                                self.assertEqual(stats['source_quality'],source)
                                self.assertEqual(stats['total_row_groups'],sum(s['row_groups'] for s in observed if s['file'].startswith(source+'/')))
                                if source=='q1':
                                    self.assertEqual(stats['index_used'],index!='off')
                                    if index=='off':
                                        self.assertEqual(stats['manifest_bytes'],0)
                                        self.assertEqual(stats['decoded_rows'],sum(s['records'] for s in observed if s['file'].startswith('q1/')))
                                    else:
                                        self.assertGreater(stats['manifest_bytes'],0)
            with p.QueryReader(path,regions,min_mapq=30) as r:
                row_output=[row for batch in r.iter_batches() for row in batch]
            self.assertEqual(row_output,self.oracle(path,regions,min_mapq=30))
            # Public StreamingReader shares automatic q0/q1 selection.
            with p.StreamingReader(path,min_mapq=30) as r:
                self.assertEqual([row for batch in r.iter_batches() for row in batch],self.oracle(path,[('chr1',0,2**33),('chr2',0,2**33)] if kind=='pairs' else regions,min_mapq=30))

    def test_q1_does_not_open_q0_or_index_and_propagates_q1_errors(self):
        path=self.pairs(name='q1-only');regions=[('chr1',0,2**33)]
        expected=self.oracle(path,regions,min_mapq=30)
        self.build_both(path)
        (self.generation(path)/'manifest.json').write_text('corrupt')
        shard=next((path/'q0').glob('*.parquet'))
        original=shard.read_bytes();shard.write_bytes(b'broken')
        for mode in ('auto','off'):
            rows,stats=self.collect(path,regions,mode,min_mapq=30)
            self.assertEqual(rows,expected)
            self.assertEqual(stats['source_quality'],'q1')
        shard.write_bytes(original)
        self.assertEqual(self.collect(path,regions,'require',min_mapq=30)[0],expected)
        (self.generation(path,'q1')/'manifest.json').write_text('corrupt')
        with self.assertRaisesRegex(RuntimeError,'required index unavailable'):
            p.QueryReader(path,regions,min_mapq=30,index='require')
        q1=next((path/'q1').glob('*.parquet'));q1.write_bytes(b'broken')
        with self.assertRaises(RuntimeError):p.QueryReader(path,regions,min_mapq=30)
        q1.unlink();(path/'q1').rmdir()
        with self.assertRaises(RuntimeError):p.QueryReader(path,regions,min_mapq=30)

    def test_complete_and_local_ids_stay_on_q0_with_positive_mapq(self):
        for local in (False,True):
            path=self.concat(name='q0-required-'+str(local),local=local)
            self.build_both(path)
            # Neither complete_reads nor shard-local mapping needs q1 at all.
            for shard in (path/'q1').glob('*.parquet'):shard.unlink()
            (path/'q1').rmdir()
            modes=('matching_alignments','complete_reads') if local else ('complete_reads',)
            for mode in modes:
                for index in ('auto','off','require'):
                    regions=[('chr1',0,2**33)]
                    rows,stats=self.collect(path,regions,index,min_mapq=30,filter_mode=mode,batch_rows=1)
                    self.assertEqual(rows,self.oracle(path,regions,min_mapq=30,filter_mode=mode))
                    self.assertEqual(stats['source_quality'],'q0')
                    self.assertFalse(stats['index_used'])

    def test_q1_index_missing_stale_corrupt_and_partition_binding(self):
        path=self.pairs(name='q1-invalidation');regions=[('chr1',0,2**33)]
        p.build_index(path)  # A valid q0 index cannot satisfy a q1 requirement.
        def unavailable():
            rows,stats=self.collect(path,regions,'auto',min_mapq=30)
            self.assertEqual(rows,self.oracle(path,regions,min_mapq=30))
            self.assertEqual(stats['source_quality'],'q1')
            self.assertFalse(stats['index_used'])
            with self.assertRaisesRegex(RuntimeError,'required index unavailable'):
                p.QueryReader(path,regions,min_mapq=30,index='require')
        unavailable()
        p.build_index(path,quality='q1')
        root=path/'.pqsio-index';q0_current=(root/'CURRENT').read_bytes()
        manifest=self.generation(path,'q1')/'manifest.json';original=manifest.read_bytes()
        for modification in ('quality','version'):
            m=json.loads(original)
            if modification=='quality':m['build']['quality']='q0'
            else:m['version']='unknown'
            manifest.write_text(json.dumps(m));unavailable();manifest.write_bytes(original)
        part=self.generation(path,'q1')/'0.rg';raw=part.read_bytes()
        part.write_bytes(raw[:-1]);unavailable();part.write_bytes(raw)
        shard=next((path/'q1').glob('*.parquet'));stat=shard.stat()
        os.utime(shard,ns=(stat.st_atime_ns,stat.st_mtime_ns+1_000_000));unavailable()
        self.assertTrue(self.collect(path,regions,'require')[1]['index_used'])
        p.build_index(path,quality='q1',rebuild=True)
        self.assertEqual((root/'CURRENT').read_bytes(),q0_current)
        self.assertTrue(self.collect(path,regions,'require',min_mapq=30)[1]['index_used'])
        with self.assertRaisesRegex(RuntimeError,'rebuild'):p.build_index(path,quality='q1')
        with self.assertRaises(ValueError):p.build_index(path,quality='q2')

    def test_q1_failed_rebuild_and_readonly_source(self):
        path=self.pairs(name='q1-rebuild');self.build_both(path)
        root=path/'.pqsio-index';q0=(root/'CURRENT').read_bytes();q1=(root/'q1'/'CURRENT').read_bytes()
        shard=next((path/'q1').glob('*.parquet'));raw=shard.read_bytes();stat=shard.stat()
        pl.read_parquet(shard).with_columns(pl.lit(0,dtype=pl.UInt64).alias('pos1')).write_parquet(shard)
        with self.assertRaisesRegex(RuntimeError,'position'):p.build_index(path,quality='q1',rebuild=True)
        self.assertEqual((root/'CURRENT').read_bytes(),q0)
        self.assertEqual((root/'q1'/'CURRENT').read_bytes(),q1)
        self.assertFalse(list((root/'q1').glob('*.partial')))
        shard.write_bytes(raw);os.utime(shard,ns=(stat.st_atime_ns,stat.st_mtime_ns))
        files=[path/'_metadata',path/'_metadata_counts',path/'_contigsizes',*list((path/'q0').glob('*.parquet')),*list((path/'q1').glob('*.parquet'))]
        before={f:(f.read_bytes(),f.stat().st_mtime_ns) for f in files}
        for f in files:f.chmod(0o444)
        try:
            # q0 and q1 have independent build locks.
            (root/'BUILD.lock').write_text('q0 build reserved')
            p.build_index(path,quality='q1',rebuild=True)
            self.assertEqual(before,{f:(f.read_bytes(),f.stat().st_mtime_ns) for f in files})
        finally:
            (root/'BUILD.lock').unlink()
            for f in files:f.chmod(0o644)

    def test_q1_pruning_and_late_failure_without_restart(self):
        for kind in ('pairs','concat'):
            path=self.root/('q1-prune-'+kind)
            if kind=='pairs':
                with p.PairsWriter(path,{'chr1':10000},chunk_size=4) as w:
                    for group in range(8):
                        w.write_batch([p.Pair(str(group),0,group*100+1,0,group*100+2,'+','-',60)]*4)
            else:
                with p.ConcatWriter(path,{'chr1':10000},chunk_size=4) as w:
                    for i in range(32):w.write_read([p.Alignment(i+1,100,0,50,'+',0,(i//4)*100,(i//4)*100+50,60,.5)])
            p.build_index(path,quality='q1')  # q0 need not have an index.
            regions=[('chr1',0,50)]
            expected,off=self.collect(path,regions,'off',min_mapq=30,batch_rows=2)
            actual,on=self.collect(path,regions,'require',min_mapq=30,batch_rows=2)
            self.assertEqual(actual,expected)
            self.assertEqual(actual,self.oracle(path,regions,min_mapq=30))
            self.assertEqual(on['source_quality'],'q1')
            self.assertLess(on['decoded_row_groups'],off['decoded_row_groups'])
            self.assertGreater(on['skipped_row_groups'],0)
            r=p.QueryReader(path,[('chr1',0,10000)],min_mapq=30,index='require',batch_rows=1)
            it=r.iter_columns();next(it)
            (self.generation(path,'q1')/'1.rg').write_bytes(b'broken')
            with self.assertRaises(RuntimeError):list(it)
            self.assertFalse(r.stats['complete']);r.close()

    def test_q1_build_is_independent_for_shard_local_concat(self):
        path=self.concat(name='q1-local-build',local=True)
        q1_names={f.name for f in (path/'q1').glob('*.parquet')}
        # Legacy Reader(min_mapq=1) would select q0 for shard-local IDs.
        # Index construction must explicitly select the requested partition.
        next((path/'q0').glob('*.parquet')).write_bytes(b'broken')
        p.build_index(path,quality='q1')
        manifest=json.loads((self.generation(path,'q1')/'manifest.json').read_text())
        self.assertEqual(manifest['build']['quality'],'q1')
        self.assertEqual({s['name'] for s in manifest['source']['shards']},q1_names)

    def test_q1_native_capability_detection(self):
        from pqsio import query
        lib=p._library()
        class OldQueryLibrary:
            def __getattr__(self,name):
                if name=='pqsio_build_index_quality':raise AttributeError(name)
                return getattr(lib,name)
        path=self.pairs(name='q1-capability')
        with patch.object(query,'_library',return_value=OldQueryLibrary()):
            p.build_index(path)  # Existing q0 ABI remains usable.
            with self.assertRaisesRegex(RuntimeError,'lacks q1 index capability'):
                p.build_index(path,quality='q1')
            with self.assertRaisesRegex(RuntimeError,'lacks q1 index capability'):
                p.QueryReader(path,[('chr1',0,50)],min_mapq=30)

    def test_capability_and_legacy_regression(self):
        from pqsio import query
        with patch.object(query,'_library',return_value=object()):
            with self.assertRaisesRegex(RuntimeError,'lacks region-query/index capability'):p.build_index('unused')
            with self.assertRaisesRegex(RuntimeError,'lacks region-query/index capability'):p.QueryReader('unused',[])
        path=self.pairs();expected=self.oracle(path,[('chr1',0,2**33),('chr2',0,2**33)])
        self.build_both(path)
        with p.Reader(path) as r:self.assertEqual([x for b in r.iter_batches() for x in b],expected)
        old=Path(__file__).resolve().parents[1]/'benchmarks/work/libpqsio-v001.so'
        if old.is_file():
            code="""
import pqsio as p,sys
with p.Reader(sys.argv[1]) as r: assert next(r.iter_batches())
for f in (lambda:p.QueryReader(sys.argv[1],[]),lambda:p.build_index(sys.argv[1])):
    try:f()
    except RuntimeError as e: assert 'lacks region-query/index capability' in str(e)
    else:raise AssertionError('missing capability check')
"""
            subprocess.run([sys.executable,'-c',code,str(path)],env=dict(os.environ,PQSIO_LIBRARY=str(old)),check=True)


if __name__=='__main__':unittest.main()
