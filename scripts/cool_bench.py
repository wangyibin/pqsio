#!/usr/bin/env python3
"""Opt-in pairs2cool comparison against CPhasing or an archived Rust library.

Run with Pixi; --baseline-python must have CPhasing's dependencies and h5py.
Data generation, output validation and warmups are excluded from measurements.
All generated files stay below tests/output, and large fixtures are temporary.
For Rust before/after runs, select --backends pqsio-baseline pqsio and supply
--baseline-library; the separate Python environment verifies Cooler outputs.
"""
import argparse
from array import array
from collections import Counter
import contextlib
import gzip
import hashlib
import io
import importlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import time

PROJECT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).resolve()


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + '\n')


def peak_mib():
    for line in Path('/proc/self/status').read_text().splitlines():
        if line.startswith('VmHWM:'):
            return int(line.split()[1]) / 1024
    raise RuntimeError('Linux VmHWM unavailable')


def generate(config):
    import pqsio as p
    root = Path(config['root'])
    boundary = config.get('boundary', False)
    chroms = [('chr01', 25)] if boundary else [(f'chr{i:02d}', 5_000_000) for i in range(1, 9)]
    sizes = ''.join(f'{name}\t{length}\n' for name, length in chroms)
    (root / 'chrom.sizes').write_text(sizes)
    header = ('## pairs format v1.0\n#shape: upper triangle\n'
              + ''.join(f'#chromsize: {name} {length}\n' for name, length in chroms)
              + '#columns: readID chrom1 pos1 chrom2 pos2 strand1 strand2 mapq\n')
    rng = random.Random(20260919)
    expected = Counter()
    offsets, offset = [], 0
    for _, length in chroms:
        offsets.append(offset)
        offset += (length + config['bin_size'] - 1) // config['bin_size']
    offsets.append(offset)
    previous = None
    with (root / 'input.pairs').open('w') as text, \
         (root / 'input.pairs.gz').open('wb') as raw, \
         gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0, compresslevel=6) as gz, \
         p.PairsWriter(root / 'input.pqs', chroms, chunk_size=100_000) as writer:
        text.write(header)
        gz.write(header.encode())
        for start in range(0, config['rows'], 65_536):
            columns = [array(code) for code in ['I', 'Q', 'I', 'Q', 'B', 'B', 'B']]
            ids, lines = [], []
            for i in range(start, min(start + 65_536, config['rows'])):
                if boundary:
                    a, b, x, y, quality = 0, 0, [1, 10, 20][i], [10, 20, 25][i], 60
                else:
                    a = rng.randrange(len(chroms))
                    x = rng.randrange(1, chroms[a][1])
                    if i % 10 < 7:
                        b = a
                        y = min(chroms[b][1] - 1, max(1, x + int(rng.expovariate(1 / 100_000)) * rng.choice([-1, 1])))
                    else:
                        b = rng.randrange(len(chroms))
                        y = rng.randrange(1, chroms[b][1])
                    if i % 20 == 0:
                        b, y = a, x
                    # Boundary semantics are tested separately, not hidden in timing comparisons.
                    x += int(x % config['bin_size'] == 0)
                    y += int(y % config['bin_size'] == 0)
                    if (a, x) > (b, y):
                        a, b, x, y = b, a, y, x
                    if i % 10 == 9:
                        a, b, x, y = previous
                    previous = a, b, x, y
                    quality = [0, 10, 30, 60, 60][i % 5]
                if quality >= config['min_mapq']:
                    one = offsets[a] + (x - 1) // config['bin_size']
                    two = offsets[b] + (y - 1) // config['bin_size']
                    expected[min(one, two), max(one, two)] += 1
                name = f'r{i}'
                ids.append(name)
                for column, value in zip(columns, (a, x, b, y, 43, 45, quality)):
                    column.append(value)
                lines.append(f'{name}\t{chroms[a][0]}\t{x}\t{chroms[b][0]}\t{y}\t+\t-\t{quality}\n')
            block = ''.join(lines)
            text.write(block)
            gz.write(block.encode())
            writer.write_columns(p.PairColumns(*p.pack_strings(ids), *columns))
    pixel_hash = hashlib.sha256()
    bin_counts = [0] * offset
    sample = []
    for (a, b), count in sorted(expected.items()):
        pixel_hash.update(struct.pack('<qqq', a, b, count))
        bin_counts[a] += 1
        if boundary:
            sample.append([a, b, count])
    bin1_offset, nnz = [0], 0
    for count in bin_counts:
        nnz += count
        bin1_offset.append(nnz)
    result = dict(chroms=chroms, nbins=offset, nnz=len(expected), sum=sum(expected.values()),
                  pixel_sha256=pixel_hash.hexdigest(), bin_size=config['bin_size'],
                  chrom_offset=offsets, bin1_offset=bin1_offset, pixels=sample,
                  input_bytes={name: (root / name).stat().st_size for name in ['input.pairs', 'input.pairs.gz']})
    result['input_bytes']['input.pqs'] = sum(path.stat().st_size for quality in ('q0', 'q1')
                                           for path in (root / 'input.pqs' / quality).glob('*.parquet'))
    save(root / 'expected.json', result)


def worker(config):
    os.sched_setaffinity(0, config['cpus'][:config['threads']])
    if config['backend'].startswith('pqsio'):
        import pqsio
        from pqsio.cli import main
        pqsio._library()
        args = ['convert', config['input'], '--mode', 'pairs2cool', '--bin-size', str(config['bin_size']),
                '--min-mapq', str(config['min_mapq']), '--threads', str(config['threads']),
                '--chunk-size', str(config['chunk_size']), '-o', config['output']]
        def run():
            with contextlib.redirect_stdout(io.StringIO()):
                status = main(args)
            if status:
                raise RuntimeError(f'pqsio exit code {status}')
    else:
        from cphasing.cli import cli
        # Preload the callback's lazy dependencies, just as the candidate loads
        # its native library before timing. End-to-end still includes startup.
        for name in ['cooler.cli.cload', 'cphasing.core', 'cphasing.pqs', 'cphasing.utilities']:
            importlib.import_module(name)
        args = ['pairs2cool', config['input'], config['sizes'], config['output'],
                '--binsize', str(config['bin_size']), '--min-mapq', str(config['min_mapq']),
                '--threads', str(config['threads']),
                '--low-memory' if config['backend'] == 'cphasing-low' else '--no-low-memory']
        def run():
            cli.main(args=args, standalone_mode=False)
    cpu_start, start = time.process_time(), time.perf_counter()
    run()
    seconds, cpu_seconds = time.perf_counter() - start, time.process_time() - cpu_start
    save(config['metrics'], dict(seconds=seconds, cpu_seconds=cpu_seconds, peak_mib=peak_mib(),
                                python=sys.version, args=args))


def verify(path, expected_path):
    import h5py
    import numpy as np
    expected = json.loads(Path(expected_path).read_text())
    errors, hashes = [], {}
    with h5py.File(path, 'r') as f:
        names = [v.decode() for v in f['chroms/name'][:]]
        lengths = f['chroms/length'][:].tolist()
        if list(map(list, zip(names, lengths))) != expected['chroms']:
            errors.append('chromosome dictionary')
        starts, ends, ids = [], [], []
        for i, (_, length) in enumerate(expected['chroms']):
            for start in range(0, length, expected['bin_size']):
                ids.append(i); starts.append(start); ends.append(min(start + expected['bin_size'], length))
        for key, values in [('bins/chrom', ids), ('bins/start', starts), ('bins/end', ends),
                            ('indexes/chrom_offset', expected['chrom_offset']),
                            ('indexes/bin1_offset', expected['bin1_offset'])]:
            data = f[key][:].astype('<i8')
            hashes[key] = hashlib.sha256(data.tobytes()).hexdigest()
            if not np.array_equal(data, values):
                errors.append(key)
        digest, total, sample = hashlib.sha256(), 0, []
        rows = len(f['pixels/count'])
        for start in range(0, rows, 65_536):
            data = np.column_stack([f['pixels/' + key][start:start + 65_536]
                                    for key in ['bin1_id', 'bin2_id', 'count']]).astype('<i8')
            digest.update(data.tobytes())
            total += int(data[:, 2].sum())
            if expected['nbins'] <= 10:
                sample.extend(data.tolist())
        if digest.hexdigest() != expected['pixel_sha256']:
            errors.append('pixels')
        if (total, rows) != (expected['sum'], expected['nnz']):
            errors.append('counts')
        for key, value in [('bin-size', expected['bin_size']), ('sum', expected['sum']),
                            ('nbins', expected['nbins']), ('nnz', expected['nnz'])]:
            if f.attrs.get(key) != value:
                errors.append('attribute ' + key)
        layout = {key: dict(dtype=str(f[key].dtype), compression=f[key].compression,
                            compression_opts=f[key].compression_opts, chunks=f[key].chunks)
                  for key in ['pixels/bin1_id', 'pixels/count', 'bins/start']}
    print(json.dumps(dict(matches=not errors, errors=errors, sum=total, nnz=rows,
                          pixel_sha256=digest.hexdigest(), hashes=hashes, pixels=sample, layout=layout)))


def run_one(config, directory, python, env, consumer):
    config = dict(config, output=str(directory / 'out.cool'), metrics=str(directory / 'metrics.json'))
    path = directory / 'run.json'
    save(path, config)
    command = [str(python), str(SCRIPT), '--worker', str(path)]
    # GNU time captures exec-based high-water RSS and waited-for descendants;
    # worker VmHWM separately identifies the main process (verification is excluded).
    with (directory / 'stdout.log').open('w') as stdout, (directory / 'stderr.log').open('w') as stderr:
        start = time.perf_counter()
        result = subprocess.run(['/usr/bin/time', '-f', '%M', '-o', str(directory / 'rss.txt'), *command],
                                cwd=directory, env=env, stdout=stdout, stderr=stderr, timeout=600)
        end_to_end = time.perf_counter() - start
    if result.returncode:
        raise RuntimeError((directory / 'stderr.log').read_text()[-6000:])
    metrics = json.loads(Path(config['metrics']).read_text())
    checked = subprocess.run([str(consumer), str(SCRIPT), '--verify', config['output'], config['expected']],
                             env=env, capture_output=True, text=True, check=True, timeout=120)
    metrics.update(end_to_end_seconds=end_to_end, time_peak_mib=float((directory / 'rss.txt').read_text()) / 1024,
                   output_bytes=Path(config['output']).stat().st_size, verification=json.loads(checked.stdout))
    return metrics


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError('must be positive')
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-python', required=True, type=Path)
    parser.add_argument('--baseline-library', type=Path, help='archived pqsio native library for pqsio-baseline')
    parser.add_argument('--cphasing-source', required=True, type=Path)
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--rows', type=positive, nargs='+', default=[100_000, 1_000_000])
    parser.add_argument('--threads', type=positive, nargs='+', default=[1, 4])
    parser.add_argument('--repetitions', type=positive, default=3)
    parser.add_argument('--bin-size', type=positive, default=10_000)
    parser.add_argument('--min-mapq', type=int, choices=range(61), default=1)
    parser.add_argument('--chunk-size', type=positive, default=1_000_000)
    parser.add_argument('--backends', nargs='+', choices=['pqsio', 'pqsio-baseline', 'cphasing-fast', 'cphasing-low'],
                        default=['pqsio', 'cphasing-fast', 'cphasing-low'])
    parser.add_argument('--formats', nargs='+', choices=['pairs', 'gzip', 'pqs'], default=['pairs', 'gzip', 'pqs'])
    args = parser.parse_args()
    if 'pqsio-baseline' in args.backends and (args.baseline_library is None or not args.baseline_library.is_file()):
        parser.error('pqsio-baseline requires --baseline-library pointing to the previous native library')
    if args.bin_size < 2:
        parser.error('--bin-size must exceed 1 for a fixture without bin-boundary coordinates')
    env = dict(os.environ, PYTHONPATH=str(PROJECT / 'python') + os.pathsep + str(args.cphasing_source.resolve()))
    env['PATH'] = str(args.cphasing_source.resolve() / 'bin') + os.pathsep + env['PATH']
    cpus = sorted(os.sched_getaffinity(0))[:max(args.threads)]
    if len(cpus) < max(args.threads):
        parser.error('not enough CPUs in current affinity mask')
    native = Path(env['PQSIO_LIBRARY']).resolve()
    source_files = [PROJECT / 'src/cool.rs', *sorted((PROJECT / 'src/cool').glob('*.rs')),
                    PROJECT / 'src/io.rs', SCRIPT,
                    *[args.cphasing_source / 'cphasing' / name for name in ['cli.py', 'core.py', 'pqs.py', 'utilities.py']]]
    report = dict(settings={key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
                  timestamp_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                  native_sha256=sha256(native), source_sha256={str(path): sha256(path) for path in source_files},
                  cphasing_rs_sha256=sha256(args.cphasing_source / 'bin/cphasing-rs'),
                  platform=platform.platform(), cpu_model=next(line.split(':', 1)[1].strip() for line in
                      Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
                  cpus=cpus, candidate_python=sys.executable, baseline_python=str(args.baseline_python),
                  fixtures={}, boundary=[], runs=[], summary=[])
    consumer = args.baseline_python.resolve()
    versions = subprocess.run([str(consumer), '-c', 'import json,importlib.metadata as m; print(json.dumps({x:m.version(x) for x in ["cooler","h5py","polars","numpy"]}))'],
                              env=env, capture_output=True, text=True, check=True)
    report['baseline_versions'] = json.loads(versions.stdout)
    if args.baseline_library:
        report['baseline_native_sha256'] = sha256(args.baseline_library)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    root = PROJECT / 'tests/output'
    root.mkdir(exist_ok=True)
    formats = {'pairs': 'input.pairs', 'gzip': 'input.pairs.gz', 'pqs': 'input.pqs'}
    with tempfile.TemporaryDirectory(prefix='cool-bench-', dir=root) as temporary:
        temporary = Path(temporary)
        for label, rows in [('boundary', 3)] + [(str(n), n) for n in args.rows]:
            fixture = temporary / label
            fixture.mkdir()
            generation = dict(root=str(fixture), rows=rows, bin_size=10 if label == 'boundary' else args.bin_size,
                              min_mapq=args.min_mapq, boundary=label == 'boundary')
            save(fixture / 'generate.json', generation)
            subprocess.run([sys.executable, str(SCRIPT), '--generate', str(fixture / 'generate.json')],
                           env=env, check=True, timeout=600)
            expected = json.loads((fixture / 'expected.json').read_text())
            report['fixtures'][label] = {key: value for key, value in expected.items() if key != 'bin1_offset'}
            for fmt in args.formats:
                for threads in ([1] if label == 'boundary' else args.threads):
                    worker_env = dict(env, POLARS_MAX_THREADS=str(threads), RAYON_NUM_THREADS=str(threads),
                                      OPENBLAS_NUM_THREADS='1', OMP_NUM_THREADS='1', MKL_NUM_THREADS='1',
                                      NUMEXPR_NUM_THREADS='1', POLARS_THREADS=str(threads))
                    # One untimed warmup per backend/format/thread count, then alternating order.
                    for repeat in ([-1] if label == 'boundary' else range(-1, args.repetitions)):
                        backends = args.backends if repeat % 2 == 0 else list(reversed(args.backends))
                        for backend in backends:
                            run_env = dict(worker_env)
                            if backend == 'pqsio-baseline':
                                run_env['PQSIO_LIBRARY'] = str(args.baseline_library.resolve())
                            config = dict(generation, cpus=cpus, threads=threads, backend=backend,
                                          input=str(fixture / formats[fmt]), sizes=str(fixture / 'chrom.sizes'),
                                          expected=str(fixture / 'expected.json'), chunk_size=args.chunk_size)
                            with tempfile.TemporaryDirectory(dir=temporary) as output:
                                metrics = run_one(config, Path(output), sys.executable if backend.startswith('pqsio') else consumer,
                                                  run_env, consumer)
                            record = dict(rows=rows, format=fmt, threads=threads, backend=backend, repeat=repeat, **metrics)
                            if label == 'boundary':
                                report['boundary'].append(record)
                            else:
                                if not metrics['verification']['matches']:
                                    raise RuntimeError(f'output mismatch: {record}')
                                if repeat >= 0:
                                    report['runs'].append(record)
                            save(args.report, report)
                            print(json.dumps(dict(fixture=label, format=fmt, threads=threads, backend=backend,
                                                  repeat=repeat, seconds=round(metrics['seconds'], 4),
                                                  peak_mib=round(metrics['peak_mib'], 1),
                                                  matches=metrics['verification']['matches'])), flush=True)
            if label != 'boundary':
                shutil.rmtree(fixture)
    groups = {}
    for row in report['runs']:
        groups.setdefault((row['rows'], row['format'], row['threads'], row['backend']), []).append(row)
    for (rows, fmt, threads, backend), runs in groups.items():
        summary = dict(rows=rows, format=fmt, threads=threads, backend=backend,
                       repetitions=len(runs), verified=all(r['verification']['matches'] for r in runs))
        for field in ['seconds', 'cpu_seconds', 'end_to_end_seconds', 'peak_mib', 'time_peak_mib', 'output_bytes']:
            summary[field] = statistics.median(row[field] for row in runs)
        summary['seconds_range'] = [min(r['seconds'] for r in runs), max(r['seconds'] for r in runs)]
        report['summary'].append(summary)
    save(args.report, report)


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] in ('--worker', '--generate'):
        config = json.loads(Path(sys.argv[2]).read_text())
        (worker if sys.argv[1] == '--worker' else generate)(config)
    elif len(sys.argv) > 1 and sys.argv[1] == '--verify':
        verify(*sys.argv[2:])
    else:
        main()
