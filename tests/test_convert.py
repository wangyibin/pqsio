"""Small concat conversion fixtures; outputs stay in the repository."""
from pathlib import Path
import tempfile
import unittest
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)


class Convert(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)
        self.input = self.root / 'concat'
        with p.ConcatWriter(self.input, {'a': 2**34, 'b': 100}, chunk_size=2) as w:
            w.write_read([
                p.Alignment(7, 100, 40, 50, '-', 1, 10, 20, 60, .9),
                p.Alignment(7, 100, 0, 10, '+', 0, 0, 1, 0, .9),
                p.Alignment(7, 100, 20, 30, '+', 0, 2**33, 2**33+10, 30, .9),
            ])
            w.write_read([p.Alignment(9, 100, 0, 10, '+', 0, 1, 10, 60, .9)])

    def tearDown(self):
        self.tmp.cleanup()

    def rows(self, path, min_mapq=0):
        with p.Reader(path, min_mapq=min_mapq) as reader:
            return [row for batch in reader.iter_batches() for row in batch]

    def test_expand_and_batch_invariance(self):
        outputs = []
        for size in (1, 2, 100):
            output = self.root / f'pairs{size}'
            result = p.convert(self.input, output, batch_rows=size, chunk_size=size).to_dict()
            self.assertEqual(result['output_reads'], 1)
            self.assertEqual(result['counts'], {'q0_records': 3, 'q1_records': 1})
            rows = self.rows(output)
            outputs.append(rows)
            self.assertEqual([r.read_id for r in rows], ['7:0:1', '7:0:2', '7:1:2'])
            self.assertEqual([(r.pos1, r.pos2) for r in rows],
                             [(1, 2**33+6), (1, 16), (2**33+6, 16)])
            self.assertEqual([r.mapq for r in rows], [0, 0, 30])
            self.assertEqual((rows[2].chrom1, rows[2].chrom2, rows[2].strand2), (0, 1, '-'))
            self.assertEqual(self.rows(output, 1), [rows[2]])
        self.assertEqual(outputs[0], outputs[1])
        self.assertEqual(outputs[0], outputs[2])

    def test_filters_and_empty(self):
        for i, (kwargs, expected) in enumerate([
            ({'min_mapq': 1, 'max_order': 3}, 1),
            ({'max_order': 3}, 0),
            ({'min_mapq': 61}, 0),
            ({'min_order': 4}, 0),
        ]):
            output = self.root / f'filtered{i}'
            p.convert(self.input, output, batch_rows=1, **kwargs)
            self.assertEqual(len(self.rows(output)), expected)
        empty = self.root / 'empty'
        with p.ConcatWriter(empty, {'a': 100}):
            pass
        output = self.root / 'empty_pairs'
        p.convert(empty, output)
        self.assertEqual(self.rows(output), [])

    def test_validation_and_existing_output(self):
        output = self.root / 'output'
        for kwargs in ({'mode': 'unknown'}, {'min_mapq': 256}, {'min_mapq': -1},
                       {'chunk_size': 0}, {'batch_rows': True}, {'min_order': 1},
                       {'threads': 0}, {'threads': -1}, {'threads': True}, {'threads': 1.5},
                       {'min_order': 3, 'max_order': 3}):
            with self.assertRaises(ValueError):
                p.convert(self.input, output, **kwargs)
            self.assertFalse(output.exists())
        p.convert(self.input, output)
        before = self.rows(output)
        with self.assertRaises(RuntimeError):
            p.convert(self.input, output)
        self.assertEqual(before, self.rows(output))
        with self.assertRaises(RuntimeError):
            p.convert(output, self.root / 'wrong_kind')
        self.assertFalse((self.root / 'wrong_kind.partial').exists())

    def test_copy_numbers(self):
        p.set_copy_numbers(self.input, {'a': 3})
        output = self.root / 'cn_pairs'
        p.convert(self.input, output)
        self.assertEqual((output / 'cn.info').read_text(), (self.input / 'cn.info').read_text())

    def test_parallel_matches_serial(self):
        source = self.root / 'many_reads'
        with p.ConcatWriter(source, {'a': 1000}, chunk_size=40) as writer:
            for read_id in range(12):
                writer.write_read([
                    p.Alignment(read_id, 500, j*10, j*10+5, '+', 0,
                                j*10, j*10+5, 0 if j % 3 == 0 else 30, .9)
                    for j in reversed(range(2 + read_id % 7))
                ])
        for min_mapq in (0, 1):
            baseline = self.root / f'serial{min_mapq}'
            expected = p.convert(source, baseline, min_mapq=min_mapq,
                                 chunk_size=17).to_dict()
            for threads, batch_rows in ((2, 16), (4, 1000), (32, 1000), (4, 1)):
                with self.subTest(threads=threads, batch_rows=batch_rows, min_mapq=min_mapq):
                    output = self.root / f'parallel{min_mapq}_{threads}_{batch_rows}'
                    observed = p.convert(source, output, threads=threads, batch_rows=batch_rows,
                                         min_mapq=min_mapq, chunk_size=17).to_dict()
                    self.assertEqual(observed['counts'], expected['counts'])
                    self.assertEqual(observed['output_reads'], expected['output_reads'])
                    self.assertEqual(self.rows(output), self.rows(baseline))
                    self.assertEqual(self.rows(output, 1), self.rows(baseline, 1))
                    self.assertEqual((output / '_metadata_counts').read_bytes(),
                                     (baseline / '_metadata_counts').read_bytes())
                    self.assertEqual(p.validate(output, level='full').to_dict()['status'], 'valid')

    def test_parallel_stable_ties_and_copy_numbers(self):
        source = self.root / 'ties'
        with p.ConcatWriter(source, {'a': 100}, chunk_size=4) as writer:
            for read_id in range(3):
                writer.write_read([
                    p.Alignment(read_id, 100, 0, 5, strand, 0, pos, pos+1, 30, .9)
                    for pos, strand in ((20, '-'), (10, '+'), (30, '-'))
                ])
        p.set_copy_numbers(source, {'a': 2})
        for threads in (1, 4):
            output = self.root / f'ties{threads}'
            p.convert(source, output, threads=threads, chunk_size=2)
            self.assertEqual([(r.pos1, r.pos2) for r in self.rows(output)],
                             [(21, 11), (21, 31), (11, 31)] * 3)
            self.assertEqual((output / 'cn.info').read_bytes(), (source / 'cn.info').read_bytes())

    def test_parallel_empty(self):
        output = self.root / 'parallel_empty'
        result = p.convert(self.input, output, threads=4, min_mapq=255).to_dict()
        self.assertEqual(result['output_reads'], 0)
        self.assertEqual(self.rows(output), [])

    def test_decode_failure_cleans_staging(self):
        shard = next((self.input / 'q0').glob('*.parquet'))
        shard.write_bytes(b'invalid parquet')
        output = self.root / 'failed'
        with self.assertRaises(RuntimeError):
            p.convert(self.input, output)
        self.assertFalse(output.exists())
        self.assertFalse((self.root / 'failed.partial').exists())


if __name__ == '__main__':
    unittest.main()
