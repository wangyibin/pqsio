"""Exercise the CLI in subprocesses with small repository-local datasets."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import pqsio as p


ROOT = Path(__file__).resolve().parent / 'output'
ROOT.mkdir(exist_ok=True)
BINARY = Path(os.environ.get("PQSIO_BINARY", Path(__file__).resolve().parents[1] / "target/dev-release/pqsio"))


class CliRunner:
    def run_cli(self, *args, code=0, without_library=False):
        env = os.environ.copy()
        if without_library:
            env['PQSIO_LIBRARY'] = str(ROOT / 'missing-cli-library.so')
        result = subprocess.run(
            [str(BINARY), *map(str, args)],
            env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, code, result.stdout + result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        return result


class CliTests(CliRunner, unittest.TestCase):
    def test_region_string_parser_and_errors(self):
        for value in ['chr1', ':0-10', 'chr1:10-10', 'chr1:20-10', 'chr1:-1-10',
                      'chr1:0-x', 'chr1:0-18446744073709551616', 'chr1:0-10-20']:
            result = self.run_cli('query', 'input', '--region', value, code=2, without_library=True)
            self.assertIn('CHROM:START-END', result.stderr)
            self.assertNotIn('missing-cli-library', result.stderr)

    def test_help_and_version_without_library(self):
        for args in [(), ('--help',), ('--version',),
                     *((command, '--help') for command in
                       ('convert', 'inspect', 'validate', 'subset', 'merge', 'info', 'head', 'view', 'export'))]:
            with self.subTest(args=args):
                result = self.run_cli(*args, without_library=True)
                self.assertIn('pqsio', result.stdout)
                self.assertEqual(result.stderr, '')
        self.assertEqual(self.run_cli('--version', without_library=True).stdout.strip(),
                         f'pqsio {p.__version__}')

    def test_usage_errors_before_loading_library(self):
        for args in [
            ('unknown',), ('convert', 'input'), ('inspect',), ('merge', '-o', 'out'),
            ('convert', 'input', '-o', 'out', '--mode', 'pairs2concat'),
            ('convert', 'input', '-o', 'out', '--mode', 'pairs2cool'),
            ('convert', 'input', '-o', 'out', '--mode', 'pairs2cool', '--bin-size', '0'),
            ('convert', 'input', '-o', 'out', '--min-mapq', '256'),
            ('convert', 'input', '-o', 'out', '--threads', '0'),
            ('convert', 'input', '-o', 'out', '--min-order', '3', '--max-order', '3'),
            ('subset', 'input', '-o', 'out', '--region', 'a:10-10'),
            ('subset', 'input', '-o', 'out', '--region', 'a:x-20'),
            ('subset', 'input', '-o', 'out', '--read-id', '001', '--read-index', '1'),
            ('validate', 'input', '--max-issues', '0'),
            ('merge', 'input', '-o', 'out', '--batch-rows', '-1'),
        ]:
            with self.subTest(args=args):
                result = self.run_cli(*args, code=2, without_library=True)
                self.assertIn('error', result.stderr.lower())
                self.assertEqual(result.stdout, '')
                self.assertNotIn('missing-cli-library', result.stderr)

    def test_no_native_shared_library_needed(self):
        result = self.run_cli('inspect', 'input', code=1, without_library=True)
        self.assertNotIn('missing-cli-library', result.stderr)
        self.assertEqual(result.stdout, '')

    def test_invalid_bin_sizes_before_loading_library(self):
        for size in ['0k', '-1m', '', 'k', '10kb', 'NaN', 'inf', '0.0001k',
                     '9223372036854775808', '9223372036854775.808k', '1e1000000000g']:
            with self.subTest(size=size):
                result = self.run_cli('convert', 'input', '-o', 'out', '--mode', 'pairs2cool',
                                      '--bin-size='+size, code=2, without_library=True)
                self.assertIn('error', result.stderr.lower())
                self.assertNotIn('missing-cli-library', result.stderr)

    def test_native_help_aliases(self):
        for command in [(), ('convert',), ('inspect',), ('validate',), ('subset',), ('merge',)]:
            expected = self.run_cli(*command, '--help', without_library=True).stdout
            for alias in ['-h', '-help']:
                self.assertEqual(self.run_cli(*command, alias, without_library=True).stdout, expected)
        self.assertNotIn('--samtools', self.run_cli('convert', '--help').stdout)

    def test_executable_is_native(self):
        with BINARY.open('rb') as stream:
            self.assertEqual(stream.read(4), b'\x7fELF')



class NativeCliTests(CliRunner, unittest.TestCase):
    def test_native_cli_without_python_or_shared_library(self):
        env = dict(os.environ, PATH='/nonexistent', PYTHONPATH='/nonexistent', PQSIO_LIBRARY='/nonexistent')
        result = subprocess.run([str(BINARY), 'inspect', str(self.source)], env=env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['metadata']['format'], 'concat')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root / '输入 concat.pqs'
        with p.ConcatWriter(self.source, {'a': 100, 'b': 100}, chunk_size=2) as writer:
            writer.write_read([
                p.Alignment(7, 100, 0, 10, '+', 0, 0, 1, 0, .9),
                p.Alignment(7, 100, 20, 30, '+', 0, 10, 20, 30, .9),
                p.Alignment(7, 100, 40, 50, '-', 1, 20, 30, 60, .9),
            ])
            writer.write_read([p.Alignment(9, 100, 0, 10, '+', 0, 1, 10, 60, .9)])

    def records(self, path):
        with p.Reader(path) as reader:
            return [row for batch in reader.iter_batches() for row in batch]

    def test_plain_json_and_empty_selection_defaults(self):
        for color in [{'NO_COLOR': '1'}, {'FORCE_COLOR': '1'}]:
            with patch.dict(os.environ, dict(color, TERM='xterm-256color')):
                result = self.run_cli('inspect', self.source)
                self.assertEqual(result.stderr, '')
                self.assertNotIn('\x1b[', result.stdout)
                self.assertEqual(json.loads(result.stdout)['metadata']['format'], 'concat')
        output = self.root / 'all.pqs'
        self.run_cli('subset', self.source, '-o', output)
        self.assertEqual(self.records(output), self.records(self.source))

    def test_convert_defaults_and_filters(self):
        output = self.root / 'pairs.pqs'
        report = json.loads(self.run_cli('convert', self.source, '-o', output).stdout)
        self.assertEqual(report['counts'], {'q0_records': 3, 'q1_records': 1})
        rows = self.records(output)
        self.assertEqual([r.read_id for r in rows], ['7:0:1', '7:0:2', '7:1:2'])
        self.assertEqual([(r.pos1, r.pos2) for r in rows], [(1, 16), (1, 26), (16, 26)])

        filtered = self.root / 'filtered.pqs'
        result = self.run_cli('convert', self.source, '-o', filtered,
                              '--mode', 'concat2pairs', '--min-mapq', '1',
                              '--min-order', '2', '--max-order', '3', '-t', '2',
                              '--batch-rows', '1', '--chunk-size', '1')
        self.assertEqual(json.loads(result.stdout)['counts']['q0_records'], 1)
        self.assertEqual(self.records(filtered), [p.Pair('7:0:1', 0, 16, 1, 26, '+', '-', 30)])
        self.assertEqual(p.validate(filtered, 'full').status, 'valid')

    def test_inspect_and_valid_report(self):
        report = json.loads(self.run_cli('inspect', self.source).stdout)
        self.assertEqual(report['metadata']['format'], 'concat')
        self.assertEqual(report['observed']['records']['q0'], 4)
        for level in ('quick', 'full'):
            result = self.run_cli('validate', self.source, '--level', level, '--max-issues', '1')
            self.assertEqual(json.loads(result.stdout)['status'], 'valid')

    def test_invalid_and_incomplete_exit_status(self):
        counts = self.source / '_metadata_counts'
        original = counts.read_text()
        counts.write_text('q0_records\t99\nq1_records\t3\n')
        result = self.run_cli('validate', self.source, code=1)
        self.assertEqual(json.loads(result.stdout)['status'], 'invalid')
        counts.write_text(original)
        metadata = self.source / '_metadata'
        metadata.write_text(metadata.read_text().replace("'0.2.0'", "'9.0.0'"))
        result = self.run_cli('validate', self.source, code=3)
        self.assertEqual(json.loads(result.stdout)['status'], 'incomplete')

    def test_subset_concat_complete_reads(self):
        output = self.root / 'subset.pqs'
        result = self.run_cli('subset', self.source, '-o', output,
                              '--chrom', 'a', '--chrom', 'b',
                              '--region', 'a:10-20', '--region', 'b:20-30',
                              '--read-index', '7', '--min-mapq', '30',
                              '--mode', 'complete-reads', '--no-provenance',
                              '--batch-rows', '1', '--chunk-size', '1')
        self.assertEqual(json.loads(result.stdout)['counts']['q0_records'], 3)
        self.assertEqual(self.records(output), self.records(self.source)[:3])
        self.assertFalse((output / '_subset.json').exists())

    def test_subset_pairs_keeps_string_ids_and_endpoint_mode(self):
        source = self.root / 'numeric-ids.pqs'
        rows = [p.Pair('001', 0, 1, 0, 10, '+', '-', 30),
                p.Pair('1', 0, 1, 0, 10, '+', '-', 30)]
        with p.PairsWriter(source, {'a': 100}) as writer:
            writer.write_batch(rows)
        for mode, expected in [('either', rows[:1]), ('both', [])]:
            output = self.root / mode
            self.run_cli('subset', source, '-o', output, '--read-id', '001',
                         '--region', 'a:0-1', '--pairs-mode', mode)
            self.assertEqual(self.records(output), expected)
            self.assertTrue((output / '_subset.json').exists())

    def test_merge_preserves_input_order(self):
        sources = [self.root / name for name in ('z.pqs', 'a.pqs')]
        rows = [p.Pair(name, 0, 1, 0, 10, '+', '-', 30) for name in ('z', 'a')]
        for source, row in zip(sources, rows):
            with p.PairsWriter(source, {'a': 100}) as writer:
                writer.write_batch([row])
        output = self.root / 'merged.pqs'
        result = self.run_cli('merge', *sources, '-o', output,
                              '--no-provenance', '--batch-rows', '1', '--chunk-size', '1')
        self.assertEqual(json.loads(result.stdout)['counts']['q0_records'], 2)
        self.assertEqual(self.records(output), rows)
        self.assertFalse((output / '_merge_sources.jsonl').exists())

    def test_native_errors_and_existing_output(self):
        result = self.run_cli('inspect', self.root / 'missing', code=1)
        self.assertIn('error', result.stderr.lower())
        self.assertEqual(result.stdout, '')
        output = self.root / 'existing.pqs'
        self.run_cli('convert', self.source, '-o', output)
        before = self.records(output)
        result = self.run_cli('convert', self.source, '-o', output, code=1)
        self.assertEqual(result.stdout, '')
        self.assertEqual(self.records(output), before)
        self.assertFalse(Path(str(output) + '.partial').exists())


if __name__ == '__main__':
    unittest.main()
