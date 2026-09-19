#!/usr/bin/env python3
"""Opt-in real-PQS Cooler benchmark; native conversion, streamed h5py checks.

Input remains unchanged. Each full conversion is sequential and its output is
removed after validation. Reports/profiles stay under tests/output. No cache
flushing or input copying. Profiling runs are separate from ordinary timings.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

from cool_bench import peak_mib, save, sha256

PROJECT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).resolve()


def manifest(source):
    return dict(metadata={name: sha256(source / name) for name in
                          ['_metadata', '_metadata_counts', '_contigsizes']},
                shards=[dict(name=p.name, size=p.stat().st_size, mtime_ns=p.stat().st_mtime_ns)
                        for p in sorted((source / 'q0').glob('*.parquet'))])


def worker(config):
    os.sched_setaffinity(0, config['cpus'])
    import pqsio
    pqsio._library()
    cpu, wall = time.process_time(), time.perf_counter()
    result = pqsio.convert(config['input'], config['output'], mode='pairs2cool',
                           bin_size=config['bin_size'], min_mapq=config['min_mapq'],
                           chunk_size=config['chunk_size'], threads=config['threads']).to_dict()
    save(config['metrics'], dict(seconds=time.perf_counter()-wall,
                                 cpu_seconds=time.process_time()-cpu,
                                 peak_mib=peak_mib(), conversion=result))


def verify(config):
    import hashlib
    import h5py
    import numpy as np
    sizes = [line.split()[:2] for line in (Path(config['input']) / '_contigsizes').read_text().splitlines()
             if line.strip()]
    sizes = [(name, int(length)) for name, length in sizes]
    chrom_bins = [(length + config['bin_size'] - 1) // config['bin_size'] for _, length in sizes]
    nbins = sum(chrom_bins)
    result = json.loads(Path(config['metrics']).read_text())['conversion']
    counts = dict(line.split() for line in (Path(config['input']) / '_metadata_counts').read_text().splitlines()
                  if line.strip())
    assert result['input_records'] == int(counts['q0_records'])
    if config['min_mapq'] in [0, 1]:
        assert result['sum'] == int(counts['q{}_records'.format(config['min_mapq'])])
    digest = hashlib.sha256()
    with h5py.File(config['output'], 'r') as f:
        assert [x.decode() for x in f['chroms/name'][:]] == [n for n, _ in sizes]
        assert f['chroms/length'][:].tolist() == [n for _, n in sizes]
        assert int(f.attrs['nbins']) == nbins
        chrom_offset = np.concatenate(([0], np.cumsum(chrom_bins, dtype=np.int64)))
        assert np.array_equal(f['indexes/chrom_offset'][:], chrom_offset)
        # Check each chromosome's full bins, without allocating a dense matrix.
        bin_chrom, bin_start, bin_end = (f['bins/'+name] for name in ['chrom', 'start', 'end'])
        for chrom, ((_, length), count) in enumerate(zip(sizes, chrom_bins)):
            start, end = int(chrom_offset[chrom]), int(chrom_offset[chrom+1])
            starts = np.arange(count, dtype=np.int64) * config['bin_size']
            assert np.all(bin_chrom[start:end] == chrom)
            assert np.array_equal(bin_start[start:end], starts)
            assert np.array_equal(bin_end[start:end], np.minimum(starts+config['bin_size'], length))
        nnz, total, previous = len(f['pixels/count']), 0, None
        histogram = np.zeros(nbins, dtype=np.int64)
        for start in range(0, nnz, 1_000_000):
            pixels = np.column_stack([f['pixels/'+name][start:start+1_000_000]
                                      for name in ['bin1_id', 'bin2_id', 'count']]).astype('<i8')
            a, b, c = pixels.T
            assert np.all((0 <= a) & (a <= b) & (b < nbins) & (c > 0))
            assert np.all((a[1:] > a[:-1]) | ((a[1:] == a[:-1]) & (b[1:] > b[:-1])))
            first, last = (int(a[0]), int(b[0])), (int(a[-1]), int(b[-1]))
            assert previous is None or first > previous
            previous = last
            histogram += np.bincount(a, minlength=nbins)
            total += int(c.sum())
            digest.update(pixels.tobytes())
        offsets = np.concatenate(([0], np.cumsum(histogram)))
        assert np.array_equal(f['indexes/bin1_offset'][:], offsets)
        assert total == result['sum'] == int(f.attrs['sum'])
        assert nnz == result['nnz'] == int(f.attrs['nnz'])
        assert int(f.attrs['bin-size']) == config['bin_size']
        layout = {name: dict(dtype=str(f['pixels/'+name].dtype), chunks=f['pixels/'+name].chunks,
                            compression=f['pixels/'+name].compression,
                            level=f['pixels/'+name].compression_opts)
                  for name in ['bin1_id', 'bin2_id', 'count']}
    print(json.dumps(dict(checked=True, pixel_sha256=digest.hexdigest(), nnz=nnz,
                          sum=total, nbins=nbins, layout=layout)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--consumer-python', type=Path, required=True)
    parser.add_argument('--library', type=Path, default=Path(os.environ['PQSIO_LIBRARY']))
    parser.add_argument('--threads', type=int, nargs='+', default=[1, 4, 10])
    parser.add_argument('--repetitions', type=int, default=1)
    parser.add_argument('--chunk-size', type=int, default=1_000_000)
    parser.add_argument('--bin-size', type=int, default=20_000)
    parser.add_argument('--min-mapq', type=int, default=1)
    parser.add_argument('--profile', action='store_true', help='perf sampling; exclude from ordinary timing comparisons')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if min(args.threads + [args.repetitions, args.chunk_size, args.bin_size]) < 1:
        parser.error('threads/repetitions/chunk-size/bin-size must be positive')
    root = PROJECT / 'tests/output'
    report_path = args.report.resolve()
    if not report_path.is_relative_to(root):
        parser.error('--report must be under tests/output')
    if report_path.exists():
        parser.error('report already exists; choose a new path')
    source = args.input.resolve()
    library = args.library.resolve()
    cpus = sorted(os.sched_getaffinity(0))
    if max(args.threads) > len(cpus):
        parser.error('not enough CPUs in current affinity mask')
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report = dict(timestamp_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                  settings={key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
                  native_sha256=sha256(library), script_sha256=sha256(SCRIPT),
                  source_sha256={str(p.relative_to(PROJECT)): sha256(p) for p in
                                 [PROJECT/'src/cool.rs', *sorted((PROJECT/'src/cool').glob('*.rs'))]},
                  input=str(source), manifest=manifest(source), platform=platform.platform(), runs=[])
    save(report_path, report)
    for repeat in range(args.repetitions):
        for threads in (args.threads if repeat % 2 == 0 else list(reversed(args.threads))):
            with tempfile.TemporaryDirectory(prefix='cool-real-', dir=root) as temporary:
                directory = Path(temporary)
                config = dict(input=str(source), output=str(directory/'out.cool'), metrics=str(directory/'metrics.json'),
                              bin_size=args.bin_size, min_mapq=args.min_mapq, chunk_size=args.chunk_size,
                              threads=threads, cpus=cpus[:threads])
                path = directory/'config.json'
                save(path, config)
                env = dict(os.environ, PQSIO_LIBRARY=str(library), POLARS_MAX_THREADS=str(threads),
                           RAYON_NUM_THREADS=str(threads), OPENBLAS_NUM_THREADS='1', OMP_NUM_THREADS='1',
                           NUMEXPR_NUM_THREADS='1', MKL_NUM_THREADS='1')
                command = [sys.executable, str(SCRIPT), '--worker', str(path)]
                log = report_path.with_name(report_path.stem+'-t{}-r{}.log'.format(threads, repeat))
                if args.profile:
                    profile = log.with_suffix('.perf.data')
                    command = ['perf', 'record', '-q', '-F', '99', '-e', 'cpu-clock:u', '--call-graph', 'dwarf,8192',
                               '-o', str(profile), '--', *command]
                started = time.perf_counter()
                with log.open('w') as stderr:
                    subprocess.run(command, env=env, stderr=stderr, check=True, timeout=3600)
                wall = time.perf_counter()-started
                metrics = json.loads(Path(config['metrics']).read_text())
                check_started = time.perf_counter()
                verified = subprocess.run([str(args.consumer_python), str(SCRIPT), '--verify', str(path)],
                                          env=env, capture_output=True, text=True, check=True, timeout=600)
                verification = json.loads(verified.stdout)
                if report['runs']:
                    assert verification['pixel_sha256'] == report['runs'][0]['verification']['pixel_sha256']
                assert manifest(source) == report['manifest'], 'input changed during the benchmark'
                record = dict(threads=threads, cpus=config['cpus'], repeat=repeat, process_seconds=wall,
                              output_bytes=Path(config['output']).stat().st_size, verification=verification,
                              verification_seconds=time.perf_counter()-check_started, **metrics)
                report['runs'].append(record)
                save(report_path, report)
                print(json.dumps(record), flush=True)


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] in ['--worker', '--verify']:
        (worker if sys.argv[1] == '--worker' else verify)(json.loads(Path(sys.argv[2]).read_text()))
    else:
        main()
