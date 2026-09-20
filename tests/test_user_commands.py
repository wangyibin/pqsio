"""Direct conversions, native QC summaries, query CLI and index management."""
import gzip
import json
from pathlib import Path
import tempfile
import unittest

import pqsio as p
from test_cli import CliRunner, ROOT


class UserCommandTests(CliRunner, unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.pairs = self.root / 'pairs.pqs'
        with p.PairsWriter(self.pairs, {'a': 2000000, 'b': 2000000}, chunk_size=2) as w:
            w.write_batch([
                p.Pair('zero', 0, 1, 0, 1, '+', '+', 0),
                p.Pair('cis1', 0, 1, 0, 1001, '+', '-', 30),
                p.Pair('trans', 0, 11, 1, 21, '-', '+', 60),
                p.Pair('cis2', 1, 1, 1, 1000001, '-', '-', 20),
            ])
        self.concat = self.root / 'concat.pqs'
        with p.ConcatWriter(self.concat, {'a': 2000000, 'b': 2000000}, chunk_size=2) as w:
            w.write_read([
                p.Alignment(7, 100, 0, 10, '+', 0, 0, 10, 0, .9),
                p.Alignment(7, 100, 20, 30, '-', 1, 20, 30, 30, .8),
                p.Alignment(7, 100, 40, 50, '+', 0, 40, 50, 60, 1.0),
            ])
            w.write_read([p.Alignment(9, 200, 0, 10, '+', 0, 10, 20, 0, .5)])

    def test_new_help_without_native_library(self):
        for args in [('stats',), ('query',), ('index',), ('index', 'build'),
                     ('index', 'status'), ('index', 'rebuild'),
                     *((mode,) for mode in ['bam2pairs', 'bam2concat', 'paf2pairs',
                                           'paf2concat', 'concat2pairs', 'pairs2cool'])]:
            with self.subTest(args=args):
                result = self.run_cli(*args, '--help', without_library=True)
                self.assertIn('Usage:', result.stdout)
                self.assertEqual(result.stderr, '')
        self.assertNotIn('--min-order', self.run_cli('pairs2cool', '--help').stdout)
        self.assertNotIn('--bin-size', self.run_cli('bam2pairs', '--help').stdout)
        self.assertNotIn('--mode', self.run_cli('concat2pairs', '--help').stdout.split('Options:')[1])

    def test_real_direct_conversions(self):
        target = self.root / 'converted.pqs'
        result = self.run_cli('concat2pairs', self.concat, '-o', target)
        self.assertEqual(json.loads(result.stdout)['counts']['q0_records'], 3)
        self.run_cli('pairs2cool', self.pairs, '-o', self.root / 'pairs.cool', '--bin-size', '10k')
        self.assertTrue((self.root / 'pairs.cool').is_file())
        paf = self.root / 'tiny.paf'
        paf.write_text('read\t100\t0\t10\t+\ta\t1000\t0\t10\t10\t10\t60\n'
                       'read\t100\t20\t30\t-\tb\t1000\t20\t30\t10\t10\t30\n')
        for mode, count in [('paf2pairs', 1), ('paf2concat', 2)]:
            result = self.run_cli(mode, paf, '-o', self.root / mode)
            self.assertEqual(json.loads(result.stdout)['counts']['q0_records'], count)

    def test_usage_failures_before_native_load(self):
        for args in [('pairs2cool', 'in', '-o', 'out'),
                     ('bam2pairs', 'in', '-o', 'out', '--bin-size', '10k'),
                     ('query', 'in'), ('query', 'in', '--region', 'a:10-10'),
                     ('stats', 'in', '--min-mapq', '256'), ('index', 'build', 'in', '--quality', 'bad')]:
            self.run_cli(*args, code=2, without_library=True)

    def test_pairs_statistics_exact_and_filtered(self):
        report = json.loads(self.run_cli('stats', self.pairs, '--json').stdout)
        self.assertEqual((report['scanned_records'], report['selected_records']), (4, 4))
        self.assertEqual((report['cis_records'], report['trans_records']), (3, 1))
        self.assertEqual(report['cis_distance_bp'], {'0-999': 1, '1000-9999': 1, '10000-99999': 0,
                                                    '100000-999999': 0, '1000000+': 1})
        self.assertEqual(report['cis_fraction'], .75)
        self.assertEqual(sum(row['count'] for row in report['per_contig']), 8)
        self.assertEqual(report['strand_pairs'], {'++': 1, '+-': 1, '-+': 1, '--': 1})
        report = p.stats(self.pairs, min_mapq=30).to_dict()
        self.assertEqual((report['scanned_records'], report['selected_records'], report['filtered_records']), (4, 2, 2))
        self.assertEqual(sum(report['mapq_histogram']), 2)
        self.assertEqual(report['cis_fraction'], .5)
        self.assertIn('PQS quality statistics', self.run_cli('stats', self.pairs).stdout)

    def test_concat_statistics_read_boundaries(self):
        report = p.stats(self.concat).to_dict()
        self.assertEqual((report['scanned_reads'], report['selected_reads']), (2, 2))
        self.assertEqual(report['read_order_histogram'], {'1': 1, '3': 1})
        self.assertEqual(report['mean_read_length'], 150)
        self.assertEqual(report['multi_contig_reads'], 1)
        self.assertEqual(report['aligned_reference_bases'], 40)
        self.assertAlmostEqual(report['mean_identity'], .8, places=6)
        self.assertEqual(sum(report['filter_reasons'].values()), 4)
        report = p.stats(self.concat, min_mapq=30).to_dict()
        self.assertEqual((report['scanned_reads'], report['selected_reads']), (2, 1))
        self.assertEqual(report['read_order_histogram'], {'2': 1})
        self.assertEqual(report['mean_read_length'], 100)
        self.assertEqual(report['aligned_reference_bases'], 20)

    def test_no_selected_records_have_null_ratios(self):
        for source in [self.pairs, self.concat]:
            report = p.stats(source, min_mapq=255).to_dict()
            self.assertEqual(report['selected_records'], 0)
            self.assertIsNone(report['mapq_zero_fraction'])
            self.assertEqual(sum(report['mapq_histogram']), 0)
        empty = self.root / 'empty.pqs'
        with p.PairsWriter(empty, {'a': 100}):
            pass
        report = p.stats(empty).to_dict()
        self.assertEqual(report['scanned_records'], 0)
        self.assertIsNone(report['selected_fraction'])

    def test_index_lifecycle_quality_and_staleness(self):
        status = json.loads(self.run_cli('index', 'status', self.pairs).stdout)
        self.assertEqual([s['status'] for s in status['indexes']], ['missing', 'missing'])
        self.assertFalse((self.pairs / '.pqsio-index').exists())
        status = json.loads(self.run_cli('index', 'build', self.pairs).stdout)
        self.assertEqual([s['status'] for s in status['indexes']], ['valid', 'valid'])
        pointer = self.pairs / '.pqsio-index' / 'CURRENT'
        old = pointer.read_text()
        self.run_cli('index', 'build', self.pairs, '--quality', 'q0', code=1)
        self.assertEqual(pointer.read_text(), old)
        self.run_cli('index', 'rebuild', self.pairs, '--quality', 'q0')
        self.assertNotEqual(pointer.read_text(), old)
        metadata = self.pairs / '_metadata_counts'
        metadata.write_text(metadata.read_text() + '\n')
        self.assertEqual(p.index_status(self.pairs).to_dict()['status'], 'invalid')
        self.run_cli('index', 'build', self.pairs, '--rebuild')
        self.assertEqual(p.index_status(self.pairs, quality='q1').to_dict()['status'], 'valid')

    def test_query_index_fallback_and_exact_coordinates(self):
        args = ('query', self.pairs, '--region', 'a:10-11', '--columns', 'readID,pos1')
        fallback = self.run_cli(*args, '--show-stats', '--no-build-index')
        self.assertEqual(fallback.stdout, 'readID\tpos1\ntrans\t11\n')
        diag = json.loads(fallback.stderr)
        self.assertFalse(diag['index_used'])
        self.assertTrue(diag['complete'])
        self.assertEqual(diag['returned_rows'], 1)
        failed = self.run_cli(*args, '--index', 'require', code=1)
        self.assertEqual(failed.stdout, '')
        self.run_cli('index', 'build', self.pairs, '--quality', 'q0')
        self.assertEqual(self.run_cli(*args, '--index', 'require').stdout, fallback.stdout)
        self.assertEqual(self.run_cli(*args, '--index', 'off').stdout, fallback.stdout)
        self.run_cli(*args, '--min-mapq', '30', '--index', 'require', code=1)
        self.run_cli('index', 'build', self.pairs, '--quality', 'q1')
        indexed = self.run_cli(*args, '--min-mapq', '30', '--index', 'require', '--show-stats')
        self.assertEqual(indexed.stdout, fallback.stdout)
        self.assertEqual(json.loads(indexed.stderr)['source_quality'], 'q1')
        self.assertTrue(json.loads(indexed.stderr)['index_used'])
        both = self.run_cli(*args, '--pairs-mode', 'both')
        self.assertEqual(len(both.stdout.splitlines()), 1)

    def test_query_concat_complete_reads_and_gzip(self):
        args = ('query', self.concat, '--region', 'b:20-21', '--min-mapq', '30')
        self.assertEqual(len(self.run_cli(*args).stdout.splitlines()), 2)
        result = self.run_cli(*args, '--mode', 'complete-reads', '--show-stats')
        self.assertEqual(len(result.stdout.splitlines()), 4)
        self.assertEqual(json.loads(result.stderr)['source_quality'], 'q0')
        target = self.root / 'selected.concat.gz'
        result = self.run_cli(*args, '--mode', 'complete-reads', '--format', 'concat', '-o', target)
        self.assertEqual(json.loads(result.stdout)['records'], 3)
        self.assertEqual(len(gzip.open(target, 'rt').read().splitlines()), 3)
        result = self.run_cli(*args, '--mode', 'complete-reads', '-n', '1', '--show-stats')
        self.assertFalse(json.loads(result.stderr)['complete'])

    def test_index_status_detects_corrupt_partition(self):
        p.build_index(self.concat, quality='q1')
        self.assertEqual(p.index_status(self.concat, quality='q1').to_dict()['status'], 'valid')
        root = self.concat / '.pqsio-index' / 'q1'
        generation = root / (root / 'CURRENT').read_text().strip()
        (generation / '0.rg').write_bytes(b'corrupt')
        result = p.index_status(self.concat, quality='q1').to_dict()
        self.assertEqual(result['status'], 'invalid')
        self.assertIn('corrupt', result['reason'])

    def test_query_builds_only_needed_missing_index(self):
        args = ('query', self.pairs, '--region', 'a:10-11', '--show-stats')
        self.run_cli(*args, '--index', 'require', code=1)
        self.run_cli(*args, '--index', 'off')
        self.assertFalse((self.pairs / '.pqsio-index').exists())
        result = self.run_cli(*args, '--min-mapq', '30')
        self.assertTrue(json.loads(result.stderr)['index_used'])
        self.assertEqual(p.index_status(self.pairs, quality='q1').to_dict()['status'], 'valid')
        self.assertEqual(p.index_status(self.pairs).to_dict()['status'], 'missing')
        pointer = self.pairs / '.pqsio-index' / 'q1' / 'CURRENT'
        old = pointer.read_text()
        self.run_cli(*args, '--min-mapq', '30')
        self.assertEqual(pointer.read_text(), old)
        result = self.run_cli(*args)
        self.assertTrue(json.loads(result.stderr)['index_used'])
        self.assertEqual(p.index_status(self.pairs).to_dict()['status'], 'valid')
        pointer = self.pairs / '.pqsio-index' / 'CURRENT'
        pointer.write_text('invalid-generation')
        result = self.run_cli(*args)
        self.assertFalse(json.loads(result.stderr)['index_used'])
        self.assertEqual(pointer.read_text(), 'invalid-generation')

    def test_query_does_not_build_for_complete_reads_or_invalid_options(self):
        self.run_cli('query', self.concat, '--region', 'a:0-10', '--mode', 'complete-reads')
        self.assertFalse((self.concat / '.pqsio-index').exists())
        self.run_cli('query', self.pairs, '--region', 'missing:0-10', code=1)
        self.run_cli('query', self.pairs, '--region', 'a:0-10', '--columns', 'bogus', code=1)
        self.assertFalse((self.pairs / '.pqsio-index').exists())

    def test_auto_build_failure_keeps_scan_opt_out(self):
        root = self.pairs / '.pqsio-index'
        root.mkdir()
        (root / 'BUILD.lock').write_text('another builder')
        args = ('query', self.pairs, '--region', 'a:10-11')
        result = self.run_cli(*args, code=1)
        self.assertEqual(result.stdout, '')
        self.assertIn('automatic index build failed', result.stderr)
        self.assertIn('trans', self.run_cli(*args, '--no-build-index').stdout)
        self.assertEqual((root / 'BUILD.lock').read_text(), 'another builder')


if __name__ == '__main__':
    unittest.main()
