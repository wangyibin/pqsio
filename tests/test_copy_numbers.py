"""Tiny CN fixtures, native API and independent compatibility checks."""
import ast
import ctypes as C
import hashlib
import json
import logging
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import pqsio as p

ROOT = Path(__file__).parent / 'output'
ROOT.mkdir(exist_ok=True)


class CopyNumbers(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.root = Path(self.tmp.name)
        self.serial = 0

    def tearDown(self):
        self.tmp.cleanup()

    def dataset(self, cn=None, concat=False):
        self.serial += 1
        path = self.root / str(self.serial)
        cls = p.ConcatWriter if concat else p.PairsWriter
        with cls(path, {'a': 100, 'b': 100, 'unused': 100}, copy_numbers=cn) as w:
            if concat:
                w.write_read([p.Alignment(7, 20, 0, 10, '+', 0, 1, 11, 30, 1.),
                              p.Alignment(7, 20, 10, 20, '-', 1, 20, 30, 0, .5)])
            else:
                w.write_batch([p.Pair('same', 0, 1, 1, 20, '+', '-', 30)])
        return path

    def test_read_write_defaults_and_empty(self):
        path = self.dataset()
        info = p.read_copy_numbers(path)
        self.assertFalse(info.present)
        self.assertEqual(info.explicit, {})
        self.assertEqual(info.effective('unused'), 1)
        with self.assertRaisesRegex(ValueError, 'Unknown contig'):
            info.effective('typo')
        p.update_copy_numbers(path, {})
        self.assertFalse((path/'cn.info').exists())
        p.set_copy_numbers(path, {})
        self.assertTrue(p.read_copy_numbers(path).present)
        self.assertEqual((path/'cn.info').read_bytes(), b'')
        (path/'cn.info').write_text('  # comment\n\n b  3\n a\t1\n')
        self.assertEqual(p.read_copy_numbers(path).explicit, {'a': 1, 'b': 3})
        p.update_copy_numbers(path, {'unused': 2**64-1})
        self.assertEqual((path/'cn.info').read_bytes(), b'a\t1\nb\t3\nunused\t18446744073709551615\n')
        before = (path/'cn.info').stat()
        p.update_copy_numbers(path, {})
        self.assertEqual((path/'cn.info').stat().st_mtime_ns, before.st_mtime_ns)
        p.set_copy_numbers(path, {'b': 1})
        self.assertEqual(p.read_copy_numbers(path).explicit, {'b': 1})

    def test_invalid_and_local_diagnostics(self):
        path = self.dataset()
        cases = [('a 0', 'CN_VALUE'), ('a -1', 'CN_VALUE'), ('a x', 'CN_VALUE'),
                 ('a 18446744073709551616', 'CN_VALUE'), ('a 1\na 1', 'CN_DUPLICATE'),
                 ('typo 3', 'CN_UNKNOWN_CONTIG'), ('a 1 extra', 'CN_FORMAT')]
        for text, code in cases:
            with self.subTest(text=text):
                (path/'cn.info').write_text('# header\n'+text+'\n')
                with self.assertRaisesRegex(RuntimeError, r'cn.info:[23] contig'):
                    p.read_copy_numbers(path)
                report = p.validate(path).to_dict()
                self.assertEqual(report['status'], 'invalid')
                issue = next(i for i in report['issues'] if i['code'] == code)
                self.assertTrue(issue['file'].endswith('cn.info'))
                self.assertGreaterEqual(issue['row'], 2)
                self.assertTrue(issue['field'])
                self.assertIsNotNone(report['inspection'])
                self.assertEqual(p.inspect(path).to_dict()['copy_numbers']['status'], 'invalid')
        (path/'cn.info').unlink()
        (path/'cn.info').mkdir()
        self.assertEqual(p.validate(path).status, 'incomplete')
        self.assertEqual(p.inspect(path).to_dict()['copy_numbers']['status'], 'unreadable')

    def test_updates_protect_core_links_and_failures(self):
        path = self.dataset({'a': 3})
        def core():
            return {str(f.relative_to(path)): hashlib.sha256(f.read_bytes()).hexdigest()
                    for f in path.rglob('*') if f.is_file() and f.name != 'cn.info'}
        p.build_index(path)
        before = core()
        for value in [0, -1, 1.5, True, 2**64]:
            with self.assertRaises(ValueError):
                p.set_copy_numbers(path, {'a': value})
        with self.assertRaisesRegex(RuntimeError, 'unknown contig'):
            p.update_copy_numbers(path, {'typo': 2})
        self.assertEqual(p.read_copy_numbers(path).explicit, {'a': 3})
        shared = self.root/'shared'
        os.link(path/'cn.info', shared)
        p.update_copy_numbers(path, {'a': 1})
        self.assertEqual(shared.read_text(), 'a\t3\n')
        (path/'cn.info').unlink()
        (path/'cn.info').symlink_to(shared.resolve())
        with self.assertRaisesRegex(RuntimeError, 'linked cn.info'):
            p.set_copy_numbers(path, {'a': 2})
        self.assertEqual(shared.read_text(), 'a\t3\n')
        (path/'cn.info').unlink()
        alias = self.root/'alias'
        alias.symlink_to(path.resolve(), target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, 'symlink dataset'):
            p.set_copy_numbers(alias, {'a': 2})
        with self.assertRaisesRegex(RuntimeError, 'symlink dataset'):
            p.set_copy_numbers(str(alias)+'/', {'a': 2})
        (path/'cn.info').mkdir()
        with self.assertRaises(RuntimeError):
            p.set_copy_numbers(path, {'a': 2})
        self.assertTrue((path/'cn.info').is_dir())
        self.assertEqual(list(path.glob('.cn.info.*.partial')), [])
        (path/'cn.info').rmdir()
        p.set_copy_numbers(path, {'a': 3})
        self.assertEqual(core(), before)
        with p.QueryReader(path, regions=[('a', 0, 50)], index='require') as reader:
            list(reader.iter_columns())
        self.assertEqual(p.validate(path, 'full').status, 'valid')

    def test_writer_failure(self):
        path = self.root/'failed'
        with self.assertRaises(RuntimeError):
            p.PairsWriter(path, {'a': 10}, copy_numbers={'typo': 2})
        self.assertFalse(path.exists())
        self.assertFalse(Path(str(path)+'.partial').exists())
        from pqsio.copy_numbers import _writer_set
        w = p.PairsWriter(path, {'a': 10})
        Path(str(path)+'.partial', 'cn.info').mkdir()
        with self.assertRaises(RuntimeError):
            _writer_set(w, {'a': 2})
        with self.assertRaises(RuntimeError):
            w.finish()
        self.assertFalse(path.exists())
        self.assertFalse(Path(str(path)+'.partial').exists())

    def test_merge(self):
        for left, right, expected in [(None, None, None), ({}, None, {}),
                                      ({'a': 3}, {}, {'a': 3}),
                                      ({'a': 3}, {'a': 3}, {'a': 3}),
                                      ({'a': 1}, None, {'a': 1})]:
            a, b = self.dataset(left), self.dataset(right)
            out = self.root/f'm{self.serial}'
            result = p.merge([a, b], out).to_dict()
            info = p.read_copy_numbers(out)
            self.assertEqual(info.present, expected is not None)
            self.assertEqual(info.explicit, expected or {})
            self.assertEqual(result['sources'][0]['copy_numbers_propagated'], left is not None)
            self.assertNotIn('cn.info', result['sources'][0]['omitted_sidecars'])
        for left in [1, 2]:
            a, b = self.dataset({'a': left}), self.dataset({'a': 3})
            out = self.root/f'conflict{left}'
            with self.assertRaisesRegex(RuntimeError, 'cn.info conflict') as caught:
                p.merge([a, b], out)
            self.assertIn(str(a.resolve()), str(caught.exception))
            self.assertIn(str(b.resolve()), str(caught.exception))
            self.assertFalse(out.exists())
            self.assertFalse(Path(str(out)+'.partial').exists())

    def test_subset_modes_and_inspection(self):
        for concat in [False, True]:
            src = self.dataset({'a': 1, 'unused': 3}, concat=concat)
            for i, kwargs in enumerate([{}, {'read_ids': []}, {'min_mapq': 255},
                                        {'regions': [('unused', 0, 10)]},
                                        *([{'mode': 'complete_reads', 'min_mapq': 20}] if concat else [])]):
                out = self.root/f's{self.serial}-{i}'
                result = p.subset(src, out, **kwargs).to_dict()
                self.assertTrue(result['copy_numbers_propagated'])
                self.assertEqual(p.read_copy_numbers(out).explicit, {'a': 1, 'unused': 3})
                info = p.inspect(out).to_dict()['copy_numbers']
                self.assertEqual(info['declaration_count'], 2)
                self.assertEqual(info['default'], 1)
                self.assertEqual(p.validate(out, 'full').status, 'valid')
        empty_cn = self.dataset({})
        p.subset(empty_cn, self.root/'empty-cn-subset', read_ids=[])
        self.assertTrue(p.read_copy_numbers(self.root/'empty-cn-subset').present)
        src = self.dataset()
        out = self.root/'missing-subset'
        p.subset(src, out)
        self.assertFalse(p.read_copy_numbers(out).present)
        self.assertEqual(p.validate(out).status, 'valid')

    def test_old_library_and_c_abi(self):
        with patch.object(p, '_lib', object()):
            for action in [lambda: p.read_copy_numbers('unused'),
                           lambda: p.set_copy_numbers('unused', {}),
                           lambda: p.update_copy_numbers('unused', {})]:
                with self.assertRaisesRegex(RuntimeError, 'lacks copy-number capability'):
                    action()
        from pqsio.copy_numbers import _Entry, _function
        path = self.dataset({'a': 2})
        fn = _function('pqsio_set_copy_numbers', [C.c_char_p, C.POINTER(_Entry), C.c_size_t, C.c_uint32])
        entries = (_Entry*2)(_Entry(b'a', 1), _Entry(b'a', 1))
        self.assertEqual(fn(os.fsencode(path), entries, 2, 0), -1)
        self.assertIn(b'duplicate', p._library().pqsio_last_error())
        entries = (_Entry*1)(_Entry(b'a', 0))
        self.assertEqual(fn(os.fsencode(path), entries, 1, 0), -1)
        self.assertEqual(p.read_copy_numbers(path).explicit, {'a': 2})

    def test_actual_old_library(self):
        import subprocess
        import sys
        old = Path(__file__).resolve().parents[1]/'benchmarks/work/libpqsio-v001.so'
        if not old.is_file():
            self.skipTest('named archived v0.0.1 library unavailable')
        code = """
import pqsio as p
from pathlib import Path
import sys
root = Path(sys.argv[1])
with p.PairsWriter(root/'legacy', {'a': 100}) as w:
    w.write_batch([p.Pair('r', 0, 1, 0, 2, '+', '-', 30)])
with p.Reader(root/'legacy') as r:
    assert len(next(r.iter_batches())) == 1
for action in [lambda: p.read_copy_numbers(root/'legacy'),
               lambda: p.set_copy_numbers(root/'legacy', {}),
               lambda: p.update_copy_numbers(root/'legacy', {}),
               lambda: p.PairsWriter(root/'new', {'a': 100}, copy_numbers={})]:
    try: action()
    except RuntimeError as e: assert 'lacks copy-number capability' in str(e)
    else: raise AssertionError('expected missing capability')
assert not (root/'new').exists()
assert not (root/'new.partial').exists()
"""
        subprocess.run([sys.executable, '-c', code, str(self.root)],
                       env=dict(os.environ, PQSIO_LIBRARY=str(old)), check=True, timeout=60)

    def test_existing_cphasing_loader(self):
        # Execute the actual method AST without importing CPhasing's optional deps.
        source = Path(__file__).resolve().parents[2]/'CPhasing/cphasing/pqs.py'
        if not source.exists():
            self.skipTest('sibling CPhasing source unavailable')
        tree = ast.parse(source.read_text())
        cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == 'PQS')
        method = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == '_load_cn_info')
        namespace = {'Path': Path, 'logger': logging.getLogger(__name__)}
        exec(compile(ast.Module(body=[method], type_ignores=[]), str(source), 'exec'), namespace)
        path = self.dataset({'a': 1, 'unused': 2**64-1})
        holder = type('PQSFixture', (), {'path': str(path)})()
        self.assertEqual(namespace['_load_cn_info'](holder), dict(p.read_copy_numbers(path).explicit))


if __name__ == '__main__':
    unittest.main()
