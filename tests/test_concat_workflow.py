"""End-to-end complete-read selection and deferred expansion, on tiny fixtures."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import pqsio as p


ROOT = Path(__file__).resolve().parent / 'output'
ROOT.mkdir(exist_ok=True)
EXAMPLE = Path(__file__).resolve().parents[1] / 'examples/concat_workflow.py'


class ConcatWorkflow(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def run_example(self, name, *args, code=0):
        output = self.root / name
        run = subprocess.run([sys.executable, str(EXAMPLE), '--output', str(output), *args],
                             capture_output=True, text=True, timeout=60)
        self.assertEqual(run.returncode, code, run.stdout + run.stderr)
        self.assertNotIn('Traceback', run.stderr)
        return output, run

    def rows(self, path):
        with p.Reader(path) as reader:
            return [row for batch in reader.iter_batches() for row in batch]

    def test_anchor_keeps_all_fields_then_expands_only_eligible_fragments(self):
        output, run = self.run_example('demo', '--demo', '--expand', '--threads', '2')
        report = json.loads(run.stdout)
        selected = output / 'selected.concat.pqs'
        original = self.rows(output / 'input.concat.pqs')
        retained = self.rows(selected)
        self.assertEqual(retained, [row for row in original if row.read_idx == 7])
        self.assertEqual([row.mapping_quality for row in retained], [40, 60, 0])
        self.assertEqual(retained[-1].filter_reason, 'low')
        self.assertEqual(report['selection']['counts']['q0_concats'], 1)
        self.assertEqual(report['selection']['counts']['q0_records'], 3)
        self.assertEqual(self.rows(output / 'pairs.pqs'),
                         [p.Pair('7:0:1', 0, 126, 1, 326, '+', '-', 40)])
        self.assertEqual((selected / 'cn.info').read_bytes(),
                         (output / 'pairs.pqs/cn.info').read_bytes())
        self.assertEqual(json.loads((output / 'workflow.json').read_text()), report)
        # Re-expand saved concat without re-importing or losing low-quality data.
        p.convert(selected, output / 'all.pairs.pqs', min_mapq=0)
        self.assertEqual(len(self.rows(output / 'all.pairs.pqs')), 3)
        p.convert(selected, output / 'excluded.pairs.pqs', min_mapq=0, max_order=3)
        self.assertEqual(self.rows(output / 'excluded.pairs.pqs'), [])
        self.assertEqual(self.rows(selected), retained)

    def test_deferred_expansion_real_input_ids_and_no_matches(self):
        output, run = self.run_example('deferred', '--demo')
        self.assertIsNone(json.loads(run.stdout)['expansion'])
        self.assertFalse((output / 'pairs.pqs').exists())
        source = output / 'input.concat.pqs'
        before = self.rows(source)
        # Real-input mode has no implicit region. ID and quality predicates intersect.
        chosen, _ = self.run_example('chosen', '--input', str(source), '--read-index', '9')
        self.assertEqual(self.rows(chosen / 'selected.concat.pqs'),
                         [row for row in before if row.read_idx == 9])
        empty, run = self.run_example('empty', '--input', str(source), '--read-index', '9',
                                     '--region', 'chr1:100-200', '--expand')
        self.assertEqual(self.rows(empty / 'selected.concat.pqs'), [])
        self.assertEqual(self.rows(empty / 'pairs.pqs'), [])
        self.assertEqual(self.rows(source), before)

    def test_invalid_options_and_existing_output_are_safe(self):
        for args in [('--region', 'chr1:20-10'), ('--select-mapq', '256'),
                     ('--min-order', '4', '--max-order', '3'), ('--threads', '0')]:
            output, _ = self.run_example('invalid', '--demo', *args, code=2)
            self.assertFalse(output.exists())
        output, _ = self.run_example('existing', '--demo')
        before = (output / 'workflow.json').read_bytes()
        self.run_example('existing', '--demo', '--expand', code=1)
        self.assertEqual((output / 'workflow.json').read_bytes(), before)
        self.assertFalse((output / 'pairs.pqs').exists())
        source = output / 'input.concat.pqs'
        nested, _ = self.run_example(str(source / 'nested'), '--input', str(source), code=1)
        self.assertFalse(nested.exists())
        wrong = self.root / 'pairs'
        with p.PairsWriter(wrong, {'chr1': 1000}):
            pass
        output, _ = self.run_example('wrong', '--input', str(wrong), code=1)
        self.assertFalse(output.exists())
