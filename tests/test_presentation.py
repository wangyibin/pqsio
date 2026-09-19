"""Native browsing/export behavior on small repository-local fixtures."""
import gzip
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr

import pqsio as p
from test_cli import CliRunner, ROOT


class PresentationTests(CliRunner, unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root / 'pairs.pqs'
        with p.PairsWriter(self.source, {'a': 1000, 'b': 1000}, chunk_size=40) as w:
            w.write_batch([p.Pair(f'{i:03}', 0, i+1, 1, i+2, '+', '-', 0 if i == 0 else 30)
                           for i in range(120)])

    def test_info_summary_and_opt_in_stats(self):
        info = p.info(self.source).to_dict()
        self.assertEqual((info['q0_records'], info['q1_records']), (120, 119))
        self.assertFalse(info['stats_scanned'])
        self.assertNotIn('mapq_histogram', info)
        self.assertGreater(info['parquet_bytes'], 0)
        stats = json.loads(self.run_cli('info', self.source, '--json', '--stats').stdout)
        self.assertEqual(stats['scanned_records'], 120)
        self.assertEqual(sum(stats['mapq_histogram']), 120)
        self.assertEqual(stats['mapq_histogram'][0], 1)
        self.assertEqual(stats['mapq_histogram'][30], 119)
        table = self.run_cli('info', self.source)
        self.assertIn('PQS summary', table.stdout)
        self.assertIn('q0 records', table.stdout)
        self.assertEqual(table.stderr, '')

    def test_preview_defaults_zero_and_all(self):
        for command, n in [('head', 10), ('view', 100)]:
            result = self.run_cli(command, self.source)
            self.assertEqual(len(result.stdout.splitlines()), n+1)
            self.assertEqual(result.stderr, '')
            self.assertTrue(result.stdout.startswith('readID\tchrom1\t'))
        self.assertEqual(len(self.run_cli('head', self.source, '-n', '0').stdout.splitlines()), 1)
        self.assertEqual(len(self.run_cli('view', self.source, '--all', '--no-header').stdout.splitlines()), 120)

    def test_selection_coordinates_and_quality_partition(self):
        args = ('view', self.source, '--all', '--columns', 'pos1,readID,chrom1', '--region', 'a:10-11')
        self.assertEqual(self.run_cli(*args).stdout, 'pos1\treadID\tchrom1\n11\t010\ta\n')
        self.assertEqual(self.run_cli(*args, '--pairs-mode', 'both').stdout, 'pos1\treadID\tchrom1\n')
        filtered = self.run_cli('view', self.source, '--all', '--min-mapq', '1', '--no-header')
        self.assertEqual(len(filtered.stdout.splitlines()), 119)
        self.assertTrue(filtered.stdout.startswith('001\ta\t2\tb\t3\t'))

    def test_compressed_pairs_and_tsv_export(self):
        for suffix in ['pairs', 'pairs.gz', 'pairs.mgz']:
            output = self.root / suffix
            report = p.export(self.source, output, threads=2).to_dict()
            self.assertEqual(report['records'], 120)
            text = gzip.open(output, 'rt').read() if suffix != 'pairs' else output.read_text()
            self.assertIn('#chromsize: a 1000\n', text)
            self.assertIn('#columns: readID chrom1 pos1 chrom2 pos2 strand1 strand2 mapq\n', text)
            rows = [line for line in text.splitlines() if not line.startswith('#')]
            self.assertEqual(len(rows), 120)
            self.assertEqual(rows[0], '000\ta\t1\tb\t2\t+\t-\t0')
            self.assertFalse(Path(str(output)+'.partial').exists())
        output = self.root / 'selected.tsv'
        result = self.run_cli('export', self.source, '-o', output, '--columns', 'chrom2,pos2', '-n', '1')
        self.assertEqual(json.loads(result.stdout)['records'], 1)
        self.assertEqual(output.read_text(), 'chrom2\tpos2\nb\t2\n')
        stdout = self.run_cli('export', self.source, '-o', '-', '-n', '1')
        self.assertTrue(stdout.stdout.startswith('## pairs format'))
        self.assertNotIn('"records"', stdout.stdout)

    def test_concat_names_intervals_and_logical_reads(self):
        source = self.root / 'concat.pqs'
        with p.ConcatWriter(source, {'a': 1000, 'b': 1000}, chunk_size=2) as w:
            w.write_read([p.Alignment(7, 100, 0, 10, '+', 0, 0, 10, 0, .9),
                          p.Alignment(7, 100, 20, 30, '-', 1, 20, 30, 60, .9)])
            w.write_read([p.Alignment(9, 100, 0, 10, '+', 0, 1, 11, 60, .9)])
        stats = p.info(source, stats=True).to_dict()
        self.assertEqual((stats['scanned_records'], stats['scanned_reads']), (3, 2))
        output = self.root / 'concat.tsv'
        p.export(source, output, columns=['read_idx', 'chrom', 'start', 'end'], batch_rows=1)
        rows = output.read_text().splitlines()
        self.assertEqual(rows[1:], ['7\ta\t0\t10', '7\tb\t20\t30', '9\ta\t1\t11'])
        with self.assertRaises(RuntimeError):
            p.export(source, self.root / 'bad.pairs', format='pairs')
        for fmt in ['auto', 'concat']:
            target = self.root / f'{fmt}.concat.gz'
            result = self.run_cli('export', source, '-o', target, '--format', fmt, '-t', '2')
            self.assertEqual(json.loads(result.stdout)['format'], 'concat')
            rows = [line.split('\t') for line in gzip.open(target, 'rt').read().splitlines()]
            self.assertEqual(len(rows), 3)
            self.assertTrue(all(len(row) == 11 for row in rows))
            self.assertEqual(rows[0][:9], ['7', '100', '0', '10', '+', 'a', '0', '10', '0'])
            self.assertAlmostEqual(float(rows[0][9]), .9, places=6)
            self.assertEqual(rows[1][:9], ['7', '100', '20', '30', '-', 'b', '20', '30', '60'])
        target = self.root / 'explicit.tsv.gz'
        p.export(source, target, format='tsv')
        self.assertTrue(gzip.open(target, 'rt').read().startswith('read_idx\tread_length\t'))
        for path, kwargs in [(self.source, {}), (source, {'columns': ['read_idx']})]:
            with self.assertRaisesRegex(RuntimeError, 'concat format requires'):
                p.export(path, self.root / 'invalid.concat.gz', format='concat', **kwargs)
        self.assertFalse((self.root / 'invalid.concat.gz').exists())

    def test_output_safety_and_cleanup(self):
        output = self.root / 'existing.tsv'
        output.write_text('keep')
        with self.assertRaises(RuntimeError):
            p.export(self.source, output)
        self.assertEqual(output.read_text(), 'keep')
        partial = self.root / 'new.tsv.partial'
        partial.write_text('keep staging')
        with self.assertRaises(RuntimeError):
            p.export(self.source, self.root / 'new.tsv')
        self.assertEqual(partial.read_text(), 'keep staging')
        link = self.root / 'dangling.tsv'
        link.symlink_to(self.root / 'missing')
        with self.assertRaises(RuntimeError):
            p.export(self.source, link)
        self.assertTrue(link.is_symlink())
        with self.assertRaises(RuntimeError):
            p.export(self.source, self.source / 'out.tsv')
        malformed = self.root / 'control.pqs'
        with p.PairsWriter(malformed, {'a': 1000}) as w:
            w.write_batch([p.Pair('bad\tname', 0, 1, 0, 2, '+', '-', 30)])
        target = self.root / 'bad.tsv.gz'
        with self.assertRaisesRegex(RuntimeError, 'control characters'):
            p.export(malformed, target)
        self.assertFalse(target.exists())
        self.assertFalse(Path(str(target)+'.partial').exists())
        p.export(malformed, target, columns=['pos1'])

    def test_invalid_options(self):
        for options in [dict(limit=-1), dict(threads=0), dict(batch_rows=0), dict(columns=[]),
                        dict(columns=['pos1', 'pos1']), dict(regions=[('a', 10, 10)])]:
            with self.subTest(options=options), self.assertRaises(ValueError):
                p.export(self.source, self.root / 'bad', **options)
        with self.assertRaisesRegex(RuntimeError, 'unknown column'):
            p.export(self.source, self.root / 'bad', columns=['bogus'])
        self.assertFalse((self.root / 'bad').exists())

    def test_progress_stderr_and_callback_cleanup(self):
        result = self.run_cli('info', self.source, '--json', '--stats', '--progress')
        self.assertEqual(json.loads(result.stdout)['scanned_records'], 120)
        self.assertIn('Done in', result.stderr)
        self.assertIn('Scanning q0 MAPQ', ' '.join(result.stderr.split()))
        result = self.run_cli('export', self.source, '-o', self.root / 'out.gz', '--progress')
        self.assertEqual(json.loads(result.stdout)['records'], 120)
        self.assertIn('Finalizing text output', ' '.join(result.stderr.split()))
        self.assertEqual(self.run_cli('head', self.source, '--no-progress').stderr, '')
        from pqsio.progress import display
        error = io.StringIO()
        with redirect_stderr(error):
            with self.assertRaises(RuntimeError):
                with display(True, 'test'):
                    p.info(self.root / 'missing')
            before = error.getvalue()
            p.info(self.source, stats=True)
            self.assertEqual(error.getvalue(), before)
        self.assertIn('Stopped during', before)

    def test_broken_pipe_is_clean(self):
        process = subprocess.Popen([sys.executable, '-m', 'pqsio', 'view', str(self.source), '--all'],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        process.stdout.close()
        stderr = process.stderr.read().decode()
        process.stderr.close()
        self.assertEqual(process.wait(timeout=30), 0, stderr)
        self.assertNotIn('Traceback', stderr)

    def test_preview_stops_before_later_corrupt_shard(self):
        shards = sorted((self.source / 'q0').glob('*.parquet'))
        self.assertGreater(len(shards), 1)
        shards[-1].write_bytes(b'broken parquet')
        preview = self.run_cli('head', self.source, '-n', '1')
        self.assertEqual(len(preview.stdout.splitlines()), 2)
        output = self.root / 'failed.pairs.gz'
        self.run_cli('export', self.source, '-o', output, code=1)
        self.assertFalse(output.exists())
        self.assertFalse(Path(str(output)+'.partial').exists())

    def test_export_can_feed_native_cooler_conversion(self):
        text = self.root / 'input.pairs.gz'
        p.export(self.source, text)
        result = self.run_cli('convert', text, '--mode', 'pairs2cool', '--bin-size', '10',
                              '-o', self.root / 'out.cool', '--progress')
        report = json.loads(result.stdout)
        self.assertTrue((self.root / 'out.cool').is_file())
        self.assertIn('Merging, compressing and writing Cooler', ' '.join(result.stderr.split()))
        self.assertEqual(report['input_records'], 120)


if __name__ == '__main__':
    unittest.main()
