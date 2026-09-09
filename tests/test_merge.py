"""Small synthetic merge fixtures, all temporary outputs stay in the repository."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import polars as pl
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)


class Merge(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def pairs(self, name, contigs=None, rows=None):
        path = self.root / name
        with p.PairsWriter(path, contigs or {'a': 100}, chunk_size=1) as w:
            w.write_batch(rows if rows is not None else [p.Pair('same', 0, 1, 0, 2, '+', '-', 0)])
        return path

    def concat(self, name, ids, cuts=(), local=False):
        path = self.root / name
        with p.ConcatWriter(path, {'long': 2**34}, chunk_size=100) as w:
            w.write_read([p.Alignment(1, 100, 0, 50, '+', 0, 2**33, 2**33+50, 60, .5, '通过"\n')])
        frame = pl.read_parquet(next((path/'q0').glob('*.parquet')))
        frame = pl.concat([frame]*len(ids)) if ids else frame.head(0)
        frame = frame.with_columns(pl.Series('read_idx', ids, dtype=pl.UInt64),
            pl.Series('mapping_quality', [i % 2 for i in range(len(ids))], dtype=pl.UInt8))
        for f in (path/'q0').glob('*.parquet'):
            f.unlink()
        bounds = [0, *cuts, len(ids)]
        for i, (a, b) in enumerate(zip(bounds, bounds[1:])):
            frame.slice(a, b-a).write_parquet(path/'q0'/f'{i}.parquet', row_group_size=2)
        if local:
            meta = path/'_metadata'
            meta.write_text(meta.read_text().replace("'read_idx_scope': 'global'", "'read_idx_scope': 'shard'"))
        return path

    def records(self, path, q=0):
        with p.Reader(path, min_mapq=q) as r:
            return [row for batch in r.iter_batches() for row in batch]

    def fail(self, inputs, output=None, message=None, **kwargs):
        output = output or self.root/'failed'
        with self.assertRaisesRegex((RuntimeError, ValueError), message or '.'):
            p.merge(inputs, output, **kwargs)
        self.assertFalse(output.exists())
        self.assertFalse(Path(str(output)+'.partial').exists())

    def test_pairs_union_order_q1_and_sidecars(self):
        a = self.pairs('z', {'a': 100, 'unused': 200}, [p.Pair('same', 0, 1, 1, 2, '+', '-', 0)])
        b = self.pairs('a', {'long': 2**34, 'a': 100}, [p.Pair('same', 1, 2, 0, 2**33, '-', '+', 60)])
        empty = self.pairs('empty', {'extra': 300}, [])
        (a/'cn.info').write_text('a\t3\n')
        (a/'application.info').write_text('not propagated')
        # Broken q1 is intentionally ignored and reconstructed from q0.
        next((b/'q1').glob('*.parquet')).write_bytes(b'broken q1')
        out = self.root/'merged'
        result = p.merge([a, b, empty], out, chunk_size=1, batch_rows=1).to_dict()
        with p.Reader(out) as r:
            self.assertEqual([name for name, _ in r.contigs], ['a', 'unused', 'long', 'extra'])
        rows = self.records(out)
        self.assertEqual([r.read_id for r in rows], ['same', 'same'])
        self.assertEqual((rows[1].chrom1, rows[1].chrom2, rows[1].pos2), (0, 2, 2**33))
        self.assertEqual(len(self.records(out, 1)), 1)
        self.assertEqual(result['counts']['q0_records'], 2)
        self.assertEqual(result['sources'][0]['omitted_sidecars'], ['application.info'])
        manifest = [json.loads(s) for s in (out/'_merge_sources.jsonl').read_text().splitlines()]
        self.assertEqual(manifest[2]['input_records'], '0')
        self.assertEqual(p.read_copy_numbers(out).explicit, {'a': 3})
        self.assertEqual(p.validate(out, 'full').status, 'valid')
        self.assertEqual(pl.read_parquet(next((out/'q0').glob('*.parquet'))).schema['pos1'], pl.UInt64)

    def test_concat_boundaries_and_provenance(self):
        a = self.concat('global', [7,7,7,8,8,2**64-1], cuts=(2,4))
        b = self.concat('local', [7,7,7,7], cuts=(2,), local=True)
        c = self.concat('same', [7])
        for batch_rows in [1, 3, 100]:
            out = self.root/f'out{batch_rows}'
            result = p.merge([a,b,c], out, chunk_size=2, batch_rows=batch_rows).to_dict()
            rows = self.records(out)
            self.assertEqual([r.read_idx for r in rows], [1,1,1,2,2,3,4,4,5,5,6])
            self.assertTrue(all(r.start == 2**33 and r.filter_reason == '通过"\n' for r in rows))
            mapping = [json.loads(s) for s in (out/'_merge_reads.jsonl').read_text().splitlines()]
            self.assertEqual(mapping[0], {'source_index': 0, 'output_read_id': '1', 'logical_read_id': '7', 'raw_read_id': '7', 'shards': ['q0/0.parquet','q0/1.parquet']})
            self.assertEqual(mapping[2]['raw_read_id'], str(2**64-1))
            self.assertEqual([(r['logical_read_id'], r['raw_read_id'], r['shards']) for r in mapping[3:5]], [('1','7',['q0/0.parquet']),('2','7',['q0/1.parquet'])])
            self.assertEqual(result['counts']['q0_concats'], 6)
            self.assertEqual(result['counts']['q1_records'], 5)
            self.assertEqual(p.validate(out, 'full').status, 'valid')
            # First oversized read occupies a single oversized shard.
            self.assertEqual(pl.read_parquet(out/'q0/0.parquet').height, 3)

    def test_concat_union_and_missing_legacy_scope(self):
        a = self.concat('legacy', [0,0])
        meta = a/'_metadata'
        meta.write_text(meta.read_text().replace(" 'read_idx_scope': 'global',", ''))
        b = self.root/'different_contigs'
        with p.ConcatWriter(b, {'unused': 300, 'long': 2**34}) as w:
            w.write_read([p.Alignment(9, 100, 0, 50, '+', 0, 10, 60, 0, .5),
                          p.Alignment(9, 100, 50, 100, '-', 1, 2**33, 2**33+50, 60, .5)])
        out = self.root/'union'
        p.merge([a,b], out, batch_rows=1, provenance=False)
        rows = self.records(out)
        self.assertEqual([r.chrom for r in rows], [0,0,1,0])
        self.assertEqual([r.read_idx for r in rows], [1,1,2,2])
        self.assertFalse(list(out.glob('_merge*')))

    def test_precheck_paths_and_conflicts(self):
        a = self.pairs('source')
        alias = self.root/'alias'
        alias.symlink_to(a.resolve(), target_is_directory=True)
        self.fail([a, alias], message='duplicate input')
        self.fail([a, a.parent/'.'/a.name], message='duplicate input')
        self.fail([a], a/'nested', message='overlaps')
        self.fail([a], alias/'nested', message='overlaps')
        conflict = self.pairs('conflict', {'a': 101})
        self.fail([a, conflict], message='a length conflict.*100.*101')
        self.fail([a, self.concat('concat', [1])], message='pairs and concat')
        out = self.root/'existing'
        out.mkdir()
        with self.assertRaises(RuntimeError): p.merge([a], out)
        self.assertTrue(out.is_dir())
        partial = self.root/'reserved.partial'
        partial.mkdir()
        with self.assertRaises(RuntimeError): p.merge([a], self.root/'reserved')
        self.assertTrue(partial.is_dir())
        dangling = self.root/'dangling'
        dangling.symlink_to(self.root/'absent')
        with self.assertRaises(RuntimeError): p.merge([a], dangling)
        self.assertTrue(dangling.is_symlink())

    def test_invalid_stream_rolls_back(self):
        for i, (ids, cuts, local) in enumerate([([2,1], (), False), ([2,1], (1,), False), ([2,1], (), True), ([1,2,1], (), False)]):
            a = self.concat(f'bad{i}', ids, cuts, local)
            self.fail([a], self.root/f'fail{i}', chunk_size=1, batch_rows=1)
        a = self.concat('bad_length', [1,1], cuts=(1,))
        shard = a/'q0/1.parquet'
        pl.read_parquet(shard).with_columns(pl.lit(200, dtype=pl.UInt32).alias('read_length')).write_parquet(shard)
        self.fail([a], message='length must agree')
        a = self.pairs('corrupt')
        next((a/'q0').glob('*.parquet')).write_bytes(b'bad')
        good = self.pairs('good_before_corrupt')
        self.fail([good, a], chunk_size=1)

    def test_empty_disabled_and_parameters(self):
        a = self.concat('empty', [])
        out = self.root/'empty_out'
        result = p.merge([a], out, provenance=False).to_dict()
        self.assertEqual(result['counts']['q0_records'], 0)
        self.assertEqual(result['provenance_files'], [])
        self.assertFalse(list(out.glob('_merge*')))
        self.fail([])
        for kwargs in [{'chunk_size': 0}, {'batch_rows': -1}, {'batch_rows': 2**32}, {'chunk_size': True}]:
            self.fail([a], **kwargs)
        with patch('pqsio._library', return_value=object()):
            self.fail([a], message='lacks merge capability')
        meta = a/'_metadata'
        original = meta.read_text()
        for old, new in [("'global'", "'unknown'"), ("'0.2.0'", "'99'"), ("'porec_alignment'", "'unknown'")]:
            meta.write_text(original.replace(old, new))
            self.fail([a])


if __name__ == '__main__':
    unittest.main()
