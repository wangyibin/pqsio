"""Native Cooler conversion, with optional independent cooler/h5py verification."""
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from test_cli import BINARY
import pqsio as p

ROOT = Path(__file__).resolve().parent / 'output'
ROOT.mkdir(exist_ok=True)
CONSUMER = os.environ.get('PQSIO_COOLER_TEST_PYTHON', sys.executable)
PROBE = subprocess.run([CONSUMER, '-c', 'import cooler,h5py'], capture_output=True)
HAVE_CONSUMER = PROBE.returncode == 0

READ = '''
import json,sys,cooler,h5py
c=cooler.Cooler(sys.argv[1])
with h5py.File(sys.argv[1]) as f:
    result=dict(names=c.chromnames,lengths=c.chromsizes.tolist(),bins=c.bins()[:].assign(chrom=lambda x: x.chrom.astype(str)).values.tolist(),
      pixels=c.pixels()[:].values.tolist(),chrom_offset=f['indexes/chrom_offset'][:].tolist(),
      bin1_offset=f['indexes/bin1_offset'][:].tolist(),matrix=c.matrix(balance=False)[:].tolist(),
      attrs={key:c.info[key] for key in ['format','format-version','bin-size','bin-type','storage-mode','nnz','sum']},
      name_kind=f['chroms/name'].dtype.kind,coords=[str(f[key].dtype) for key in ['chroms/length','bins/start','bins/end']],
      compression=f['pixels/count'].compression)
print(json.dumps(result, default=lambda value: value.item()))
'''


class CoolTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def source(self):
        source = self.root / 'input.pqs'
        rows = [p.Pair('r0',0,1,0,10,'+','-',30), p.Pair('r1',0,10,0,1,'-','+',0),
                p.Pair('r2',1,1,0,11,'+','+',20), p.Pair('r3',0,25,1,12,'+','-',30),
                p.Pair('r4',0,11,0,11,'+','+',30), p.Pair('r5',1,12,1,12,'+','+',5),
                p.Pair('r6',0,25,1,12,'+','-',40)]
        with p.PairsWriter(source, {'a':25,'b':12,'unused':10}, chunk_size=2) as writer:
            writer.write_batch(rows)
        return source

    def convert(self, source, name='out.cool', **kwargs):
        output = self.root / name
        result = p.convert(source, output, 'pairs2cool', bin_size=10, **kwargs).to_dict()
        self.assertTrue(output.is_file())
        self.assertFalse(Path(str(output)+'.partial').exists())
        self.assertEqual(list(self.root.glob('.pqsio-cool-*')), [])
        return output, result

    def read(self, output):
        result = subprocess.run([CONSUMER, '-c', READ, str(output)], capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def clean(self, output):
        self.assertFalse(output.exists())
        self.assertFalse(Path(str(output)+'.partial').exists())
        self.assertEqual(list(self.root.glob('.pqsio-cool-*')), [])

    def test_pqs_aggregation_filtering_and_native_only(self):
        source = self.source()
        with patch.object(p, 'Reader', side_effect=AssertionError('Python reader')), \
             patch('subprocess.Popen', side_effect=AssertionError('subprocess')):
            output, result = self.convert(source, chunk_size=1, batch_rows=2)
        self.assertEqual((result['sum'],result['nnz'],result['nbins'],result['nchroms']), (7,5,6,3))
        self.assertEqual(result['input_records'], 7)  # q1 is not counted twice
        _, filtered = self.convert(source, 'filtered.cool', min_mapq=20, chunk_size=2)
        self.assertEqual((filtered['sum'],filtered['nnz'],filtered['skipped_mapq']), (5,4,2))
        _, empty = self.convert(source, 'empty.cool', min_mapq=255, threads=4)
        self.assertEqual((empty['sum'],empty['nnz'],empty['nbins']), (0,0,6))

    @unittest.skipUnless(HAVE_CONSUMER, 'set PQSIO_COOLER_TEST_PYTHON to Python with cooler and h5py')
    def test_independent_cooler_matrix_indexes_and_boundaries(self):
        source = self.source()
        for batch_rows, chunk_size, threads in [(1,1,1),(2,2,4),(100,100,4)]:
            output, _ = self.convert(source, f'out-{batch_rows}.cool', batch_rows=batch_rows,
                                     chunk_size=chunk_size, threads=threads)
            data = self.read(output)
            self.assertEqual(data['names'], ['a','b','unused'])
            self.assertEqual(data['bins'], [['a',0,10],['a',10,20],['a',20,25],['b',0,10],['b',10,12],['unused',0,10]])
            self.assertEqual(data['pixels'], [[0,0,2],[1,1,1],[1,3,1],[2,4,2],[4,4,1]])
            self.assertEqual(data['chrom_offset'], [0,3,5,6])
            self.assertEqual(data['bin1_offset'], [0,1,3,4,4,5,5])
            self.assertEqual(data['matrix'], [[2,0,0,0,0,0],[0,1,0,1,0,0],[0,0,0,0,2,0],
                                              [0,1,0,0,0,0],[0,0,2,0,1,0],[0,0,0,0,0,0]])
            self.assertEqual(data['attrs'], {'format':'HDF5::Cooler','format-version':3,'bin-size':10,
                                             'bin-type':'fixed','storage-mode':'symmetric-upper','nnz':5,'sum':7})
            self.assertEqual(data['name_kind'], 'S')
            self.assertEqual(data['coords'], ['int64']*3)
            self.assertEqual(data['compression'], 'gzip')
        output, _ = self.convert(source, 'empty.cool', min_mapq=255, threads=4)
        self.assertEqual(self.read(output)['bin1_offset'], [0]*7)

    def test_parallel_pqs_row_groups_and_failure_cancellation(self):
        import polars as pl
        source = self.source()
        # Reorder physical columns, split row groups, and retain long unused IDs.
        for path in (source/'q0').glob('*.parquet'):
            frame = pl.read_parquet(path).with_columns(pl.lit('long-id'*1000).alias('read_idx'))
            frame.select(list(reversed(frame.columns))).write_parquet(path, row_group_size=1)
        for threads in [1,2,4]:
            _, report = self.convert(source, f'groups-{threads}.cool', threads=threads,
                                     batch_rows=1, chunk_size=2, min_mapq=20)
            self.assertEqual((report['input_records'],report['sum'],report['nnz'],report['skipped_mapq']), (7,5,4,2))
        shard = sorted((source/'q0').glob('*.parquet'))[0]
        original = shard.read_bytes()
        frame = pl.read_parquet(shard)
        cases = [None, frame.drop('mapq'),
                 frame.with_columns(pl.lit(None, dtype=pl.UInt32).alias('pos1')),
                 frame.with_columns(pl.lit(26, dtype=pl.UInt32).alias('pos1'))]
        for i, malformed in enumerate(cases):
            if malformed is None: shard.write_bytes(b'broken parquet')
            else: malformed.write_parquet(shard, row_group_size=1)
            output = self.root / f'failed-{i}.cool'
            result = subprocess.run([str(BINARY),'convert',str(source),'--mode','pairs2cool',
                '--bin-size','10','--threads','4','--batch-rows','1','--chunk-size','1','-o',str(output)],
                capture_output=True,text=True,timeout=30)
            self.assertIn(result.returncode, [1,2], result.stderr)
            self.assertNotIn('Traceback', result.stderr)
            self.assertIn('PQS shard', result.stderr)
            self.clean(output)
            shard.write_bytes(original)

    @unittest.skipUnless(HAVE_CONSUMER, 'set PQSIO_COOLER_TEST_PYTHON to Python with cooler and h5py')
    def test_parallel_compression_full_chunks_and_padded_tail(self):
        count = 2*65536+17
        source = self.root/'chunks.pairs'
        with source.open('w') as stream:
            stream.write(f'#chromsize: a {count}\n')
            for i in range(count): stream.write(f'r{i}\ta\t1\ta\t{i+1}\t+\t-\n')
            stream.write('duplicate\ta\t1\ta\t65537\t+\t-\n')
        check = '''
import sys,h5py,numpy as np,zlib,cooler
path,n,chunk=sys.argv[1],int(sys.argv[2]),int(sys.argv[3])
assert cooler.Cooler(path).info['nnz']==n
with h5py.File(path) as f:
    expected=np.ones(n,dtype='i8'); expected[65536]=2
    assert np.all(f['pixels/bin1_id'][:]==0)
    assert np.array_equal(f['pixels/bin2_id'][:],np.arange(n))
    assert np.array_equal(f['pixels/count'][:],expected)
    assert np.array_equal(f['indexes/bin1_offset'][:],np.r_[0,np.full(n,n)])
    for name in ['bin1_id','bin2_id','count']:
        d=f['pixels/'+name]
        assert d.dtype.kind=='i' and d.dtype.itemsize==8
        assert d.chunks==(chunk,) and d.shuffle and d.compression=='gzip' and d.compression_opts==6
    last=(n//chunk)*chunk
    mask,raw=f['pixels/count'].id.read_direct_chunk((last,))
    assert mask==0
    raw=zlib.decompress(raw)
    assert len(raw)==chunk*8
    values=np.frombuffer(np.frombuffer(raw,dtype='u1').reshape(8,chunk).T.copy().tobytes(),dtype=f['pixels/count'].dtype)
    assert np.array_equal(values[:n-last],expected[last:])
    assert np.all(values[n-last:]==0)
'''
        for threads,batch in [(1,65536),(4,65536),(4,100003),(4,4096)]:
            output = self.root/f'compressed-{threads}-{batch}.cool'
            report = p.convert(source,output,'pairs2cool',bin_size=1,threads=threads,
                               batch_rows=batch,chunk_size=50000).to_dict()
            self.assertEqual((report['nnz'],report['sum']), (count,count+1))
            result = subprocess.run([CONSUMER,'-c',check,str(output),str(count),str(min(batch,65536))],
                                    capture_output=True,text=True,timeout=60)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertFalse(Path(str(output)+'.partial').exists())
            self.assertEqual(list(self.root.glob('.pqsio-cool-*')), [])

    def test_gzip_text_columns_mapq_and_unmapped(self):
        header = '#chromsize: a 25\n#chromsize: b 12\n#columns: readID chrom2 pos2 chrom1 pos1 mapq1 mapq2\n'
        source = self.root / 'input.data'
        source.write_bytes(gzip.compress((header+'r\tb\t12\ta\t25\t30\t20\n').encode())
                           + gzip.compress(b'r2\ta\t25\tb\t12\t10\t30\nu\t!\t0\ta\t1\t0\t0\n'))
        with patch('gzip.open', side_effect=AssertionError('Python gzip')):
            _, result = self.convert(source, min_mapq=15, threads=2, chunk_size=1)
        self.assertEqual((result['sum'],result['nnz'],result['skipped_mapq'],result['skipped_unmapped']), (1,1,1,1))

    def test_bare_text_sizes_empty_and_cli(self):
        source = self.root / 'input.pairs'
        source.write_text('r\tb\t12\ta\t25\t+\t-\t30\n')
        sizes = self.root / 'sizes.gz'
        sizes.write_bytes(gzip.compress(b'a\t25\nb\t12\n'))
        output = self.root / 'cli.cool'
        result = subprocess.run([str(BINARY),'convert',str(source),'--mode','pairs2cool',
                                 '--bin-size','10','--contigsizes',str(sizes),'-o',str(output)],
                                capture_output=True,text=True,timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['sum'], 1)
        source.write_text('')
        _, report = self.convert(source, 'empty.cool', contigsizes=sizes)
        self.assertEqual((report['sum'],report['nnz'],report['nbins']), (0,0,5))

    def test_humanized_bin_sizes_api_and_cli(self):
        source = self.root / 'units.pairs'
        source.write_text('#chromsize: a 2000001\nr\ta\t10001\ta\t1000001\t+\t-\n')
        for i, (size, expected) in enumerate([
            (10000, 10000), ('10000', 10000), ('10k', 10000), ('10K', 10000),
            ('1m', 1000000), ('1M', 1000000), ('1.5m', 1500000),
            ('1g', 1000000000), ('1G', 1000000000), (' 1.5 M ', 1500000),
            ('1e4', 10000), ('10.0009k', 10000),
            ('9223372036854775807', 2**63-1), ('9223372036854775.807k', 2**63-1),
        ]):
            with self.subTest(size=size):
                report = p.convert(source, self.root / f'api-{i}.cool', 'pairs2cool',
                                   bin_size=size).to_dict()
                self.assertEqual(report['bin_size'], expected)
                self.assertEqual(report['nbins'], (2000001+expected-1)//expected)
                self.assertEqual(report['sum'], 1)
        for i, (flag, size, expected) in enumerate([
            ('--bin-size', '10k', 10000), ('--binsize', '1m', 1000000),
            ('-bs', '1.5M', 1500000),
        ]):
            with self.subTest(flag=flag, size=size):
                result = subprocess.run([str(BINARY),'convert',str(source),
                    '--mode','pairs2cool',flag,size,'-o',str(self.root/f'cli-{i}.cool')],
                    capture_output=True,text=True,timeout=60)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                self.assertEqual(report['bin_size'], expected)
                self.assertEqual(report['nbins'], (2000001+expected-1)//expected)

    def test_malformed_inputs_and_cleanup(self):
        header = '#chromsize: a 25\n'
        cases = ['r\ta\t1\ta\t2\t+\t-\n', header+'bad\n', header+'r\ta\t0\ta\t2\t+\t-\n',
                 header+'r\ta\t26\ta\t2\t+\t-\n', header+'r\tb\t1\ta\t2\t+\t-\n',
                 header+'#chromsize: a 30\n', '#chromsize: 非ASCII 25\n',
                 header+'r\ta\t1\ta\t2\t+\t-\n#chromsize: b 25\n']
        for i, content in enumerate(cases):
            source = self.root / f'{i}.pairs'; source.write_text(content)
            output = self.root / f'{i}.cool'
            with self.assertRaises(ValueError): p.convert(source, output, 'pairs2cool', bin_size=10, chunk_size=1)
            self.clean(output)
        source.write_text(header+'r\ta\t1\ta\t2\t+\t-\n')
        with self.assertRaisesRegex(ValueError,'requires mapq'):
            p.convert(source, output, 'pairs2cool', bin_size=10, min_mapq=1)
        source.write_bytes(gzip.compress(source.read_bytes())[:-8])
        with self.assertRaises(RuntimeError): p.convert(source, output, 'pairs2cool', bin_size=10)
        self.clean(output)

    def test_options_existing_paths_and_concat_rejection(self):
        source = self.source()
        for kwargs in [{'bin_size':None},{'bin_size':0},{'bin_size':True},{'bin_size':'0k'},
                       {'bin_size':'nan'},{'bin_size':'10kb'},{'bin_size':'9223372036854775808'},
                       {'bin_size':'1e-1000000000'},{'bin_size':10,'min_order':2},
                       {'bin_size':10,'include_secondary':True},{'bin_size':10,'contigsizes':'sizes'}]:
            with self.assertRaises(ValueError): p.convert(source, self.root/'bad.cool', 'pairs2cool', **kwargs)
            self.clean(self.root/'bad.cool')
        for name in ['exists.cool', 'staged.cool.partial', 'link.cool']:
            path = self.root/name
            if name == 'link.cool': path.symlink_to(self.root/'missing')
            else: path.write_text('preserve')
            output = self.root/'staged.cool' if name.endswith('.partial') else path
            with self.assertRaises(ValueError): p.convert(source,output,'pairs2cool',bin_size=10)
            self.assertTrue(path.is_symlink() if name == 'link.cool' else path.read_text() == 'preserve')
        concat = self.root/'concat'
        with p.ConcatWriter(concat, {'a':25}): pass
        with self.assertRaisesRegex(ValueError, 'requires pairs PQS'):
            p.convert(concat,self.root/'concat.cool','pairs2cool',bin_size=10)
        self.clean(self.root/'concat.cool')

    @unittest.skipUnless(HAVE_CONSUMER, 'set PQSIO_COOLER_TEST_PYTHON to Python with cooler and h5py')
    def test_int64_coordinates_and_long_names(self):
        name = 'c'*300
        source = self.root/'wide.pairs'
        source.write_text(f'#chromsize: {name} {2**33+7}\nr\t{name}\t{2**33+7}\t{name}\t1\t+\t-\n')
        output = self.root/'wide.cool'
        p.convert(source,output,'pairs2cool',bin_size=2**32)
        data = self.read(output)
        self.assertEqual(data['names'], [name])
        self.assertEqual(data['pixels'], [[0,2,1]])
        self.assertEqual(data['bins'][-1], [name,2**33,2**33+7])
        pqs = self.root/'wide.pqs'
        with p.PairsWriter(pqs, {name:2**33+7}) as writer:
            writer.write_batch([p.Pair('wide',0,2**33+7,0,1,'+','-',60)])
        output = self.root/'wide-pqs.cool'
        p.convert(pqs,output,'pairs2cool',bin_size=2**32,threads=4)
        self.assertEqual(self.read(output), data)


if __name__ == '__main__': unittest.main()
