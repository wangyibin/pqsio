"""Small independent SAM/BAM and PAF fixtures for CLI/API imports."""
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import pqsio as p


ROOT = Path(__file__).resolve().parent / 'output'
ROOT.mkdir(exist_ok=True)
SAMTOOLS = shutil.which('samtools')


def paf(name='read', length=30, qstart=0, qend=10, strand='+', chrom='a',
        tlen=100, start=0, end=10, matches=9, block=10, mapq=30, tag='tp:A:P'):
    return '\t'.join(map(str, [name, length, qstart, qend, strand, chrom, tlen,
                              start, end, matches, block, mapq, tag])) + '\n'


def sam(name, flag, chrom='a', pos=1, mapq=30, cigar='10M', sequence='A'*10, tags='NM:i:0'):
    fields = [name, flag, chrom, pos, mapq, cigar, '*', 0, 0, sequence, '*']
    return '\t'.join(map(str, fields)) + ('\t' + tags if tags else '') + '\n'


class ImportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def text(self, name, content):
        path = self.root / name
        path.write_text(content)
        return path

    def bam(self, records):
        path = self.root / 'input.bam'
        header = '@HD\tVN:1.6\tSO:unsorted\n@SQ\tSN:a\tLN:100\n@SQ\tSN:b\tLN:200\n'
        result = subprocess.run([SAMTOOLS, 'view', '-b', '-o', str(path), '-'],
                                input=header + records, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return path

    def rows(self, path, q=0):
        with p.Reader(path, min_mapq=q) as reader:
            return [row for batch in reader.iter_batches() for row in batch]

    def cli(self, source, mode, *options, code=0):
        output = self.root / 'cli.pqs'
        result = subprocess.run([sys.executable, '-m', 'pqsio', 'convert', str(source),
                                 '--mode', mode, '-o', str(output), *options],
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, code, result.stdout + result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        return output, result

    def assert_clean(self, output):
        self.assertFalse(output.exists())
        self.assertFalse(Path(str(output) + '.partial').exists())
        self.assertEqual(list(self.root.glob('.pqsio-import-*')), [])

    def test_paf_unsorted_gzip_coordinates_names_and_contigs(self):
        content = (paf('z', qstart=20, qend=30, strand='-', chrom='b', tlen=200, start=50, end=60)
                   + paf('a', mapq=0) + paf('z', qstart=0, qend=10))
        source = self.root / 'compressed.data'
        source.write_bytes(gzip.compress(content.encode()))
        sizes = self.text('ref.fai', 'a\t100\t0\t0\t0\nb\t200\t0\t0\t0\nunused\t300\n')
        output, result = self.cli(source, 'paf2concat', '--contigsizes', str(sizes),
                                  '--batch-rows', '1', '--chunk-size', '1')
        report = json.loads(result.stdout)
        self.assertEqual(report['counts'], dict(q0_records=3, q1_records=2, q0_concats=2, q1_concats=1))
        rows = self.rows(output)
        self.assertEqual([(r.read_idx, r.read_start, r.read_end, r.strand, r.chrom, r.start, r.end)
                          for r in rows], [(1, 0, 10, '+', 0, 0, 10),
                                          (2, 0, 10, '+', 0, 0, 10),
                                          (2, 20, 30, '-', 1, 50, 60)])
        self.assertAlmostEqual(rows[2].identity, .9)
        with p.Reader(output) as reader:
            self.assertEqual(reader.contigs, [('a', 100), ('b', 200), ('unused', 300)])
        mapping = [json.loads(line) for line in (output / '_import_read_names.jsonl').read_text().splitlines()]
        self.assertEqual([r['read_name'] for r in mapping], ['a', 'z'])
        self.assertEqual(p.validate(output, 'full').status, 'valid')

    def test_paf_filters_secondary_and_empty(self):
        source = self.text('input.paf', paf() + paf(qstart=10, qend=20, mapq=0)
                           + paf(qstart=20, qend=30, tag='tp:A:S'))
        for name, options, count in [
            ('default', {}, 2), ('secondary', {'include_secondary': True}, 3),
            ('mapq', {'min_mapq': 1}, 1), ('order', {'min_order': 3}, 0),
            ('maxorder', {'max_order': 2}, 0),
        ]:
            with self.subTest(name=name):
                output = self.root / name
                p.convert(source, output, 'paf2concat', **options)
                self.assertEqual(len(self.rows(output)), count)
                self.assertEqual(p.validate(output, 'full').status, 'valid')
        empty = self.text('empty.paf', '')
        sizes = self.text('sizes', 'a\t100\n')
        p.convert(empty, self.root / 'empty', 'paf2concat', contigsizes=sizes)
        self.assertEqual(self.rows(self.root / 'empty'), [])

    def test_paf_pairs_direct_gzip_cli_coordinates_and_counts(self):
        content = (paf('z', qstart=20, qend=30, strand='-', start=20, end=30, mapq=0)
                   + paf('singleton')
                   + paf('z', qstart=0, qend=10, chrom='b', tlen=200, start=50, end=60)
                   + paf('z', qstart=10, qend=20))
        source = self.root / 'input.paf.gz'
        source.write_bytes(gzip.compress(content.encode()))
        sizes = self.text('sizes', 'a\t100\nb\t200\nunused\t300\n')
        # The PAF path must work without a BAM decoder on PATH.
        with patch.dict(os.environ, {'PATH': ''}):
            output, result = self.cli(source, 'paf2pairs', '--contigsizes', str(sizes),
                                      '--batch-rows', '1', '--chunk-size', '1')
        report = json.loads(result.stdout)
        self.assertEqual(report['mode'], 'paf2pairs')
        self.assertEqual(report['format'], 'pairs')
        self.assertEqual(report['counts'], dict(q0_records=3, q1_records=1))
        self.assertEqual(report['statistics']['skipped_order_groups'], 1)
        self.assertIsNone(report['read_names'])
        expected = [p.Pair('z:0:1', 0, 1, 1, 51, '+', '+', 30),
                    p.Pair('z:0:2', 0, 21, 1, 51, '-', '+', 0),
                    p.Pair('z:1:2', 0, 1, 0, 21, '+', '-', 0)]
        self.assertEqual(self.rows(output), expected)
        self.assertEqual(self.rows(output, 1), expected[:1])
        with p.Reader(output) as reader:
            self.assertEqual(reader.contigs, [('a', 100), ('b', 200), ('unused', 300)])
        self.assertEqual(p.validate(output, 'full').status, 'valid')
        self.assertFalse((output / '_import_read_names.jsonl').exists())
        self.assertEqual(list(self.root.glob('.pqsio-import-*')), [])

        plain = self.text('input.paf', content)
        # Direct import must not depend on a temporary concat dataset.
        with patch.object(p, 'ConcatWriter', side_effect=AssertionError('unexpected concat staging')):
            p.convert(plain, self.root / 'plain', 'paf2pairs', contigsizes=sizes)
        self.assertEqual(self.rows(self.root / 'plain'), expected)
        p.convert(plain, self.root / 'fiveprime', 'paf2pairs',
                  contigsizes=sizes, pair_position='five-prime')
        self.assertEqual(self.rows(self.root / 'fiveprime')[1].pos1, 30)

    def test_paf_pairs_filters_empty_and_argument_validation(self):
        source = self.text('input.paf', paf() + paf(qstart=10, qend=20, mapq=0)
                           + paf(qstart=20, qend=30, tag='tp:A:S'))
        for name, options, count in [
            ('default', {}, 1), ('secondary', {'include_secondary': True}, 3),
            ('mapq', {'min_mapq': 1}, 0), ('order', {'min_order': 3}, 0),
            ('maxorder', {'include_secondary': True, 'max_order': 3}, 0),
        ]:
            with self.subTest(name=name):
                output = self.root / name
                p.convert(source, output, 'paf2pairs', **options)
                self.assertEqual(len(self.rows(output)), count)
                self.assertEqual(p.validate(output, 'full').status, 'valid')
        for options in ({'min_order': 1}, {'max_order': 2}, {'samtools': 'x'}):
            with self.assertRaises(ValueError):
                p.convert(source, self.root / 'invalid', 'paf2pairs', **options)
            self.assert_clean(self.root / 'invalid')
        empty = self.text('empty.paf', '')
        sizes = self.text('sizes', 'a\t100\n')
        p.convert(empty, self.root / 'empty', 'paf2pairs', contigsizes=sizes)
        self.assertEqual(self.rows(self.root / 'empty'), [])
        with self.assertRaises(ValueError):
            p.convert(source, self.root / 'default', 'paf2pairs')
        self.assertEqual(len(self.rows(self.root / 'default')), 1)

    def test_native_pairs_utf8_uint64_and_batches(self):
        source = self.text('wide.paf', ''.join(
            paf('读长:q', qstart=j*10, qend=j*10+10,
                strand='-' if j == 2 else '+', tlen=2**34,
                start=2**33+j*10, end=2**33+j*10+10, mapq=0 if j == 2 else 30)
            for j in range(3)))
        expected = [p.Pair('读长:q:0:1', 0, 2**33+1, 0, 2**33+11, '+', '+', 30),
                    p.Pair('读长:q:0:2', 0, 2**33+1, 0, 2**33+21, '+', '-', 0),
                    p.Pair('读长:q:1:2', 0, 2**33+11, 0, 2**33+21, '+', '-', 0)]

        for batch_rows in (1, 2, 64):
            with self.subTest(batch_rows=batch_rows):
                output = self.root / f'batch-{batch_rows}'
                # Rust owns every record. Python writers/decoders/processes must
                # never be called by the importer, including compressed input.
                with patch.object(p, 'PairsWriter', side_effect=AssertionError('Python writer')), \
                     patch.object(p, 'ConcatWriter', side_effect=AssertionError('Python writer')), \
                     patch('subprocess.Popen', side_effect=AssertionError('subprocess')), \
                     patch('gzip.open', side_effect=AssertionError('Python gzip')):
                    result = p.convert(source, output, 'paf2pairs',
                                       batch_rows=batch_rows, chunk_size=2).to_dict()
                self.assertEqual(result['counts'], {'q0_records': 3, 'q1_records': 1})
                self.assertEqual(self.rows(output), expected)
                self.assertEqual(self.rows(output, 1), expected[:1])
                self.assertEqual(p.validate(output, 'full').status, 'valid')

    def test_columnar_pair_import_rejects_nul_ids_and_cleans_up(self):
        source = self.text('nul.paf', paf('bad\0name') + paf('bad\0name', qstart=10, qend=20))
        output = self.root / 'invalid'
        with self.assertRaisesRegex(ValueError, 'NUL'):
            p.convert(source, output, 'paf2pairs', batch_rows=1)
        self.assert_clean(output)

    def test_bad_paf_and_late_group_failure_clean_up(self):
        for i, content in enumerate([
            'bad\tline\n', paf(qend=31), paf(matches=11), paf(mapq=256),
            paf() + paf(tlen=101), paf('aaa') * 2 + paf('zzz') + paf('zzz', length=40),
        ]):
            source = self.text(f'bad{i}.paf', content)
            for mode in ('paf2concat', 'paf2pairs'):
                output = self.root / f'out{i}_{mode}'
                with self.assertRaises(ValueError):
                    p.convert(source, output, mode, batch_rows=1, chunk_size=1)
                self.assert_clean(output)
        source = self.root / 'truncated.paf.gz'
        source.write_bytes(gzip.compress(paf().encode())[:-8])
        with self.assertRaises(RuntimeError):
            p.convert(source, self.root / 'truncated', 'paf2concat')
        self.assert_clean(self.root / 'truncated')

    def test_output_protection_and_invalid_options(self):
        source = self.text('input.paf', paf())
        output = self.root / 'existing'
        output.mkdir()
        sentinel = output / 'keep'
        sentinel.write_text('unchanged')
        with self.assertRaises(ValueError):
            p.convert(source, output, 'paf2concat')
        self.assertEqual(sentinel.read_text(), 'unchanged')
        output = self.root / 'dangling'
        output.symlink_to(self.root / 'missing')
        with self.assertRaises(ValueError):
            p.convert(source, output, 'paf2concat')
        self.assertTrue(output.is_symlink())
        for options in ({'min_order': 0}, {'max_order': 1}, {'samtools': 'x'},
                        {'pair_position': 'leftmost'}, {'include_secondary': 'yes'}):
            with self.assertRaises(ValueError):
                p.convert(source, self.root / 'invalid', 'paf2concat', **options)
            self.assert_clean(self.root / 'invalid')

    def test_obsolete_samtools_option_and_missing_native_capability(self):
        source = self.text('input.bam', 'not a bam')
        with self.assertRaisesRegex(ValueError, 'omit the obsolete samtools option'):
            p.convert(source, self.root / 'obsolete', 'bam2concat', samtools='samtools')
        self.assert_clean(self.root / 'obsolete')
        source = self.text('input.paf', paf())
        class LegacyLibrary:
            def __getattr__(self, name):
                raise AttributeError(name)
        with patch.object(p, '_library', return_value=LegacyLibrary()):
            with self.assertRaisesRegex(RuntimeError, 'rebuild/update'):
                p.convert(source, self.root / 'legacy', 'paf2concat')
        self.assert_clean(self.root / 'legacy')

    def test_multimember_gzip_and_compressed_sizes(self):
        source = self.root / 'input.mgz'
        source.write_bytes(gzip.compress(paf().encode()) +
                           gzip.compress(paf(qstart=10, qend=20).encode()))
        sizes = self.root / 'sizes.gz'
        sizes.write_bytes(gzip.compress(b'a\t100\nunused\t200\n'))
        for mode in ('paf2pairs', 'paf2concat'):
            output = self.root / mode
            with patch('gzip.open', side_effect=AssertionError('Python gzip')), \
                 patch('subprocess.Popen', side_effect=AssertionError('subprocess')):
                p.convert(source, output, mode, contigsizes=sizes, threads=2)
            self.assertEqual(len(self.rows(output)), 1 if mode == 'paf2pairs' else 2)
            self.assertEqual(p.validate(output, 'full').status, 'valid')
            with p.Reader(output) as reader:
                self.assertEqual(reader.contigs, [('a', 100), ('unused', 200)])

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_bam_import_without_samtools_or_python_writers(self):
        source = self.bam(sam('pair', 65) + sam('pair', 145, chrom='b'))
        for mode in ('bam2pairs', 'bam2concat'):
            output = self.root / mode
            with patch.dict(os.environ, {'PATH': ''}), \
                 patch('subprocess.Popen', side_effect=AssertionError('subprocess')), \
                 patch.object(p, 'PairsWriter', side_effect=AssertionError('Python writer')), \
                 patch.object(p, 'ConcatWriter', side_effect=AssertionError('Python writer')):
                p.convert(source, output, mode, threads=2)
            self.assertEqual(len(self.rows(output)), 1 if mode == 'bam2pairs' else 2)
            self.assertEqual(p.validate(output, 'full').status, 'valid')

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_long_bam_clipping_reverse_identity_and_unsorted_reads(self):
        source = self.bam(
            sam('z', 0, pos=11, cigar='5S10M15S', sequence='A'*30, tags='NM:i:1')
            + sam('a', 0, pos=31, cigar='10=5I5=2D', sequence='A'*20, tags='NM:i:7')
            + sam('z', 2064, chrom='b', pos=41, cigar='10H8M12H', sequence='A'*8, tags='NM:i:2')
            + sam('secondary', 256) + sam('duplicate', 1024)
            + sam('unmapped', 4, chrom='*', pos=0, mapq=0, cigar='*'))
        output, result = self.cli(source, 'bam2concat', '--threads', '2', '--batch-rows', '1')
        report = json.loads(result.stdout)
        self.assertEqual(report['counts']['q0_records'], 3)
        self.assertEqual(report['statistics']['skipped_secondary'], 1)
        self.assertEqual(report['statistics']['skipped_duplicate_or_qcfail'], 1)
        self.assertEqual(report['statistics']['skipped_unmapped'], 1)
        rows = self.rows(output)
        self.assertEqual([(r.read_idx, r.read_length, r.read_start, r.read_end) for r in rows],
                         [(1, 20, 0, 20), (2, 30, 5, 15), (2, 30, 12, 20)])
        self.assertEqual([(r.chrom, r.start, r.end, r.strand) for r in rows],
                         [(0, 30, 47, '+'), (0, 10, 20, '+'), (1, 40, 48, '-')])
        for row, identity in zip(rows, [15/22, .9, .75]):
            self.assertAlmostEqual(row.identity, identity)
        self.assertEqual(p.validate(output, 'full').status, 'valid')

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_long_bam_eqx_skips_padding_and_read_groups(self):
        cigar = '2H3S4=1X2I3D5N1P6=4S1H'
        source = self.bam(sam('same', 0, cigar=cigar, sequence='A'*20, tags='RG:Z:one')
                          + sam('same', 0, tags='RG:Z:two\tNM:i:0'))
        output = self.root / 'out'
        p.convert(source, output, 'bam2concat')
        rows = self.rows(output)
        self.assertEqual((rows[0].read_length, rows[0].read_start, rows[0].read_end, rows[0].end),
                         (23, 5, 18, 19))
        self.assertAlmostEqual(rows[0].identity, 10/16)
        self.assertEqual([r.read_idx for r in rows], [1, 2])
        self.assertEqual(p.validate(output, 'full').status, 'valid')

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_paired_bam_pairs_expansion_mapq_and_positions(self):
        source = self.bam(sam('pair', 99, pos=11, mapq=30, tags='')
                          + sam('orphan', 65, pos=21, tags='')
                          + sam('pair', 2147, pos=31, tags='')
                          + sam('single', 0, pos=41, tags='')
                          + sam('pair', 147, chrom='b', pos=51, mapq=10, tags=''))
        output, result = self.cli(source, 'bam2pairs', '--batch-rows', '1', '--chunk-size', '1')
        self.assertEqual(self.rows(output), [
            p.Pair('pair:0:1', 0, 11, 0, 31, '+', '+', 30),
            p.Pair('pair:0:2', 0, 11, 1, 51, '+', '-', 10),
            p.Pair('pair:1:2', 0, 31, 1, 51, '+', '-', 10)])
        report = json.loads(result.stdout)
        self.assertEqual(report['statistics']['skipped_order_groups'], 2)
        self.assertEqual(p.validate(output, 'full').status, 'valid')
        p.convert(source, self.root / 'fiveprime', 'bam2pairs', pair_position='five-prime')
        self.assertEqual(self.rows(self.root / 'fiveprime')[1].pos2, 60)
        p.convert(source, self.root / 'filtered', 'bam2pairs', min_mapq=20)
        self.assertEqual(len(self.rows(self.root / 'filtered')), 1)

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_long_bam_pairs_expansion_and_canonical_endpoints(self):
        source = self.bam(sam('long', 0, chrom='b', pos=51, cigar='10M20S', sequence='A'*30, tags='')
                          + sam('long', 2048, pos=21, cigar='10H10M10H', tags='')
                          + sam('long', 2064, pos=1, mapq=0, cigar='10M20H', tags=''))
        output = self.root / 'pairs'
        p.convert(source, output, 'bam2pairs', batch_rows=1)
        self.assertEqual(self.rows(output), [
            p.Pair('long:0:1', 0, 21, 1, 51, '+', '+', 30),
            p.Pair('long:0:2', 0, 1, 1, 51, '-', '+', 0),
            p.Pair('long:1:2', 0, 1, 0, 21, '-', '+', 0)])
        self.assertEqual(len(self.rows(output, 1)), 1)
        self.assertEqual(p.validate(output, 'full').status, 'valid')

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_paired_bam_concat_preserves_each_mate_coordinates(self):
        source = self.bam(sam('pair', 65, cigar='5S10M5S', sequence='A'*20)
                          + sam('other', 0)
                          + sam('pair', 145, chrom='b', cigar='2H8M20H', sequence='A'*8)
                          + sam('pair', 2113, pos=21, cigar='15H5M', sequence='A'*5))
        output, _ = self.cli(source, 'bam2concat', '--batch-rows', '1')
        rows = self.rows(output)
        self.assertEqual([(r.read_idx, r.read_length, r.read_start, r.read_end) for r in rows],
                         [(1, 10, 0, 10), (2, 20, 5, 15), (2, 20, 15, 20), (3, 30, 20, 28)])
        names = [json.loads(line) for line in (output / '_import_read_names.jsonl').read_text().splitlines()]
        self.assertEqual([(r['read_name'], r['mate']) for r in names],
                         [('other', None), ('pair', 1), ('pair', 2)])
        self.assertEqual(p.validate(output, 'full').status, 'valid')

    @unittest.skipUnless(SAMTOOLS, 'samtools is needed to construct real BAM fixtures')
    def test_bam_missing_nm_invalid_flags_and_truncated_input(self):
        for name, records, mode, message in [
            ('nm', sam('read', 0, tags=''), 'bam2concat', 'requires an NM tag'),
            ('flags', sam('read', 1), 'bam2concat', 'READ1/READ2'),
            ('mixed', sam('read', 65) + sam('read', 0), 'bam2pairs', 'mixed paired and unpaired'),
        ]:
            with self.subTest(name=name):
                source = self.bam(records)
                with self.assertRaisesRegex(ValueError, message):
                    p.convert(source, self.root / name, mode)
                self.assert_clean(self.root / name)
        source = self.text('broken.bam', 'broken BAM')
        with self.assertRaisesRegex(RuntimeError, 'BAM'):
            p.convert(source, self.root / 'broken', 'bam2concat')
        self.assert_clean(self.root / 'broken')
        complete = self.bam(sam('pair', 65) + sam('pair', 145, chrom='b'))
        truncated = self.root / 'truncated.bam'
        # Remove the BGZF EOF block and part of the preceding data block.
        truncated.write_bytes(complete.read_bytes()[:-35])
        with self.assertRaises(RuntimeError):
            p.convert(truncated, self.root / 'truncated', 'bam2pairs', threads=2)
        self.assert_clean(self.root / 'truncated')


if __name__ == '__main__':
    unittest.main()
