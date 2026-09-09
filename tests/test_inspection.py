"""Small named fixtures for raw-data diagnostics; all work stays in repository."""
import ctypes as C
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import polars as pl
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)

class InspectionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def pairs(self, name='pairs', empty=False):
        path = self.root / name
        with p.PairsWriter(path, {'a': 100}, chunk_size=2) as w:
            if not empty:
                w.write_batch([p.Pair('甲',0,1,0,10,'+','-',0), p.Pair('',0,2,0,20,'-','+',30), p.Pair('z',0,3,0,30,'+','+',60)])
        return path

    def concat(self):
        path = self.root / 'concat'
        with p.ConcatWriter(path, {'a':100}, chunk_size=3) as w:
            w.write_reads([
                [p.Alignment(1,20,0,10,'+',0,0,10,0,0.9), p.Alignment(1,20,10,20,'-',0,10,20,30,0.8)],
                [p.Alignment(2,20,0,10,'+',0,20,30,60,0.7)],
                [p.Alignment(3,20,0,10,'+',0,30,40,0,0.7)]])
        return path

    def codes(self, report):
        return {v['code'] for v in report.issues}

    def test_inspect_and_levels(self):
        for path in (self.pairs(), self.concat(), self.pairs('empty',True)):
            info = p.inspect(path).to_dict()
            self.assertIn('record_values',info['checks_not_performed'])
            self.assertNotIn('valid',info)
            self.assertEqual(info['declared_counts']['q0_records'],info['observed']['records']['q0'])
            for level in ('quick','full'):
                r = p.validate(path,level)
                self.assertEqual(r.status,'valid',r.to_dict())
            self.assertIn('q0_q1_consistency',p.validate(path,'full').to_dict()['checks_completed'])

    def test_unknown_fields_safe_parser_and_unknown_version(self):
        path = self.pairs()
        file = path / '_metadata'
        original = file.read_text()
        file.write_text(original.rstrip()[:-1] + ", 'extension': {'中文': [None, True, 1.25]}, 'note': \"'format':'concat'\"}")
        self.assertEqual(p.inspect(path).metadata.fields['format'],'pairs')
        self.assertEqual(p.inspect(path).metadata.fields['extension']['中文'],[None,True,1.25])
        with p.Reader(path) as r:
            self.assertEqual(sum(map(len,r.iter_batches())),3)
        file.write_text(original.replace("'0.1.0'","'9.0.0'"))
        self.assertEqual(p.validate(path).status,'incomplete')
        self.assertEqual(p.inspect(path).to_dict()['coordinates'],'unknown')
        file.write_text("{'x': __import__('os').system('false')}")
        with self.assertRaises(RuntimeError): p.inspect(path)
        self.assertNotEqual(p.validate(path).status,'valid')
        file.write_text("{'is_pqs': True, 'is_pqs': False}")
        with self.assertRaises(RuntimeError): p.inspect(path)

    def test_counts_missing_schema_and_footer(self):
        path = self.pairs()
        counts = path / '_metadata_counts'
        counts.write_text('q0_records\t99\nq1_records\t2\n')
        self.assertIn('COUNT_MISMATCH',self.codes(p.validate(path)))
        counts.unlink()
        self.assertEqual(p.inspect(path).to_dict()['declared_counts'],{})
        self.assertIn('MISSING_COMPONENT',self.codes(p.validate(path)))
        (path/'q0/0.parquet').write_bytes(b'not parquet')
        self.assertNotEqual(p.validate(path).status,'valid')

    def test_coordinates_issue_cap_and_no_mutation(self):
        path = self.pairs()
        file = path/'q0/0.parquet'
        frame = pl.read_parquet(file)
        frame.with_columns(pl.lit(0,dtype=pl.UInt32).alias('pos1')).write_parquet(file,row_group_size=1)
        before = {str(f.relative_to(path)): f.read_bytes() for f in path.rglob('*') if f.is_file()}
        self.assertEqual(p.validate(path,'quick').status,'valid')
        report = p.validate(path,'full',max_issues=1)
        self.assertEqual(report.status,'invalid')
        self.assertEqual(len(report.issues),1)
        self.assertTrue(report.to_dict()['issues_truncated'])
        self.assertEqual(report.issues[0]['row'],1)
        self.assertEqual(report.issues[0]['field'],'pos1')
        self.assertEqual(before,{str(f.relative_to(path)): f.read_bytes() for f in path.rglob('*') if f.is_file()})

    def test_q1_values_and_sequence(self):
        path = self.pairs()
        file = path/'q1/0.parquet'
        frame = pl.read_parquet(file)
        frame.with_columns(pl.lit(21,dtype=pl.UInt32).alias('pos2')).write_parquet(file)
        report = p.validate(path,'full')
        self.assertEqual(report.status,'incomplete',report.to_dict())
        self.assertIn('Q0_Q1_SEQUENCE_MISMATCH',self.codes(report))
        frame.with_columns(pl.lit(0,dtype=pl.UInt8).alias('mapq')).write_parquet(file)
        self.assertEqual(p.validate(path,'full').status,'invalid')

    def test_concat_raw_ids_and_read_lengths(self):
        path = self.concat()
        file = path/'q0/0.parquet'
        frame = pl.read_parquet(file)
        frame.with_columns(pl.Series('read_length',[20,21,20],dtype=pl.UInt32)).write_parquet(file,row_group_size=1)
        report = p.validate(path,'full')
        self.assertIn('READ_LENGTH_MISMATCH',self.codes(report))
        frame.with_columns(pl.Series('read_idx',[2,2,1],dtype=pl.UInt64)).write_parquet(file,row_group_size=1)
        self.assertIn('READ_ORDER',self.codes(p.validate(path,'full')))

    def test_local_ids_and_repartitioned_global_q1(self):
        path = self.concat()
        # Change IDs identically on both sides within each named shard.
        for quality in ('q0','q1'):
            for file in (path/quality).glob('*.parquet'):
                frame = pl.read_parquet(file)
                frame.with_columns((pl.col('read_idx')-pl.col('read_idx').min()).cast(pl.UInt64)).write_parquet(file,row_group_size=1)
        meta = path/'_metadata'
        meta.write_text(meta.read_text().replace("'global'","'shard'"))
        # Equal IDs in separate shards remain distinct logical reads.
        self.assertEqual(p.validate(path,'full').status,'valid')
        path = self.pairs()
        q1 = path/'q1'
        frame = pl.concat([pl.read_parquet(f) for f in sorted(q1.glob('*.parquet'))])
        for f in q1.glob('*.parquet'): f.unlink()
        frame.write_parquet(q1/'7.parquet',row_group_size=1)
        self.assertEqual(p.validate(path,'full').status,'valid')

    def test_invalid_arguments_old_library(self):
        for n in (0,-1,True,1.2,2**100):
            with self.assertRaises(ValueError): p.validate('x',max_issues=n)
        with self.assertRaises(ValueError): p.validate('x','other')
        self.assertEqual(p.validate(self.root/'missing').status,'incomplete')
        with patch('pqsio._library',return_value=object()):
            with self.assertRaisesRegex(RuntimeError,'lacks inspection'): p.inspect('x')

    def test_ffi_callback_failure(self):
        path = self.pairs()
        from pqsio.inspection import _Callback
        lib = p._library()
        fn = lib.pqsio_inspect_json
        fn.argtypes = [C.c_char_p,_Callback,C.c_void_p]
        fn.restype = C.c_int32
        self.assertEqual(fn(str(path).encode(),_Callback(lambda *_: -1),None),-1)
        self.assertIn(b'callback failed',lib.pqsio_last_error())

    def test_null_strand_and_empty_schema(self):
        path = self.pairs()
        file = path/'q0/0.parquet'
        frame = pl.read_parquet(file)
        frame.with_columns(pl.lit(None,dtype=pl.UInt32).alias('pos1')).write_parquet(file)
        r = p.validate(path,'full')
        self.assertEqual(r.status,'invalid')
        self.assertIn('NULL_VALUES',self.codes(r))
        self.assertIn('record_values',r.to_dict()['checks_skipped'])
        frame.with_columns(pl.lit('?').cast(pl.Categorical).alias('strand1')).write_parquet(file)
        self.assertIn('INVALID_RECORD_ENCODING',self.codes(p.validate(path,'full')))
        path = self.pairs('empty-schema',True)
        meta = path/'_metadata'
        meta.write_text(meta.read_text().replace('UInt8','UInt64'))
        self.assertIn('INVALID_DECLARED_SCHEMA',self.codes(p.validate(path)))

    def test_long_positions_intervals_and_unknown_scope(self):
        path = self.root/'long'
        with p.PairsWriter(path,{'a':2**32+10}) as w:
            w.write_batch([p.Pair('x',0,2**32+1,0,2**32+2,'+','-',60)])
        self.assertEqual(p.validate(path,'full').status,'valid')
        path = self.concat()
        file = path/'q0/0.parquet'
        frame = pl.read_parquet(file)
        frame.with_columns(pl.lit(1000,dtype=pl.UInt32).alias('read_end')).write_parquet(file)
        self.assertIn('INVALID_READ_INTERVAL',self.codes(p.validate(path,'full')))
        meta = path/'_metadata'
        meta.write_text(meta.read_text().replace("'global'","'future'"))
        self.assertEqual(p.validate(path,'full').status,'incomplete')
        self.assertEqual(p.inspect(path).to_dict()['read_idx_scope'],'future')
