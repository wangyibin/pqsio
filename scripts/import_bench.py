#!/usr/bin/env python3
"""Compare importer versions (Python or native Rust) on modest synthetic PAF fixtures.

Run with Pixi. Each measurement uses a fresh process and the same native
library, four CPUs and four Polars/Rayon threads. Verification is not timed.
"""
import argparse
from array import array
import cProfile
import gzip
import hashlib
import importlib
import json
import os
from pathlib import Path
import platform
import pstats
import resource
import shutil
import statistics
import subprocess
import sys
import tempfile
import time


def positive(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError('must be positive')
    return number


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024*1024), b''):
            digest.update(block)
    return digest.hexdigest()


def peak_mib():
    # Linux's VmHWM measures this executed worker, excluding the parent's
    # historical high-water mark potentially inherited by getrusage().
    status = Path('/proc/self/status')
    if status.exists():
        return next(int(line.split()[1])/1024 for line in status.read_text().splitlines()
                    if line.startswith('VmHWM:'))
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024


def fingerprint(pqsio, path, quality, pairs):
    columns = pqsio.PairColumns if pairs else pqsio.ConcatColumns
    key = 'read_id' if pairs else 'filter_reason'
    names = [name for name, _ in columns._schema
             if not name.startswith(key) and name != 'read_offsets']
    hashes = {name: hashlib.sha256() for name in [*names, 'string_lengths', 'string_bytes']}
    records = 0
    with pqsio.Reader(path, min_mapq=quality) as reader:
        contigs = reader.contigs
        for batch in reader.iter_columns():
            records += len(getattr(batch, 'pos1' if pairs else 'read_idx'))
            for name in names:
                hashes[name].update(memoryview(getattr(batch, name)))
            offsets = getattr(batch, key + '_offsets')
            lengths = array('Q', (b-a for a, b in zip(offsets, offsets[1:])))
            hashes['string_lengths'].update(memoryview(lengths))
            hashes['string_bytes'].update(memoryview(getattr(batch, key + '_bytes')))
    return dict(records=records, contigs=contigs,
                fields={name: digest.hexdigest() for name, digest in hashes.items()})


def worker(args):
    if hasattr(os, 'sched_getaffinity'):
        os.sched_setaffinity(0, sorted(os.sched_getaffinity(0))[:4])
    import pqsio
    module = importlib.import_module('pqsio.import_alignments')
    pqsio._library()
    stages = {}
    for name in ('_store', '_write'):
        original = getattr(module, name, None)
        if original is None:
            continue

        def timed(*a, _fn=original, _name=name, **kw):
            start = time.perf_counter()
            try:
                return _fn(*a, **kw)
            finally:
                stages[_name] = time.perf_counter() - start

        setattr(module, name, timed)
    profiler = cProfile.Profile() if args.profile else None
    if profiler:
        profiler.enable()
    cpu_start = time.process_time()
    start = time.perf_counter()
    result = pqsio.convert(args.source, args.output, args.mode,
                           batch_rows=args.batch_rows, chunk_size=args.chunk_size).to_dict()
    elapsed = time.perf_counter() - start
    cpu_seconds = time.process_time() - cpu_start
    peak = peak_mib()
    if profiler:
        profiler.disable()
    result.pop('output')
    result.pop('read_names', None)
    pairs = args.mode.endswith('pairs')
    verification = dict(q0=fingerprint(pqsio, args.output, 0, pairs), q1=fingerprint(pqsio, args.output, 1, pairs),
                        validation=pqsio.validate(args.output, 'full').status)
    if not pairs:
        digest = hashlib.sha256()
        with (args.output / '_import_read_names.jsonl').open() as stream:
            for line in stream:
                digest.update(json.dumps(json.loads(line), sort_keys=True).encode())
        verification['names_sha256'] = digest.hexdigest()
    assert verification['validation'] == 'valid', verification
    profile = None
    if profiler:
        stats = pstats.Stats(profiler)
        profile = [dict(function=f'{Path(key[0]).name}:{key[1]}:{key[2]}',
                        calls=value[1], own_seconds=value[2], cumulative_seconds=value[3])
                   for key, value in sorted(stats.stats.items(), key=lambda item: item[1][3], reverse=True)[:35]]
    print(json.dumps(dict(seconds=elapsed, cpu_seconds=cpu_seconds, peak_mib=peak, stages=stages,
                          result=result, verification=verification, profile=profile)))


def fixture(path, layout, alignments, mode):
    pairs = mode.endswith('pairs')
    bam = mode.startswith('bam')
    order = 32 if layout == 'high' else 4
    reads = max(1, alignments // order)
    counts = dict(q0_records=0, q1_records=0)
    if not pairs:
        counts.update(q0_concats=0, q1_concats=0)
    for read in range(reads):
        selected = [j for j in range(order) if (read+j) % 37 != 0]
        quality = sum((read+j) % 5 != 0 for j in selected)
        counts['q0_records'] += len(selected) * (len(selected)-1) // 2 if pairs else len(selected)
        counts['q1_records'] += quality * (quality-1) // 2 if pairs else quality
        if not pairs:
            counts['q0_concats'] += int(bool(selected))
            counts['q1_concats'] += int(quality > 0)
    opener = gzip.open if layout == 'scattered-gzip' and not bam else open
    text_path = path.with_suffix('.sam') if bam else path
    with opener(text_path, 'wt', encoding='utf-8') as stream:
        if bam:
            stream.write('@HD\tVN:1.6\tSO:unsorted\n')
            for chrom in range(16):
                stream.write(f'@SQ\tSN:ctg{chrom}\tLN:2000000\n')
        groups = ((read, j) for read in range(reads) for j in range(order))
        if layout == 'scattered-gzip':
            groups = ((read, j) for j in range(order) for read in range(reads))
        for read, j in groups:
            chrom = (read+j*7) % 16
            start = ((read*101+j*1009) % 1000000) + (2**33 if chrom == 15 and not bam else 0)
            fields = [f'read{read:08d}', order*100, j*100, j*100+80,
                      '-' if j % 2 else '+', f'ctg{chrom}', 2**34, start, start+80,
                      75, 80, 0 if (read+j) % 5 == 0 else 30,
                      'tp:A:S' if (read+j) % 37 == 0 else 'tp:A:P']
            if bam:
                left, right = j*100, order*100-j*100-80
                if j % 2:
                    left, right = right, left
                cigar = (f'{left}H' if left else '') + '80M' + (f'{right}H' if right else '')
                flag = (16 if j % 2 else 0) | (2048 if j else 0) | (256 if (read+j) % 37 == 0 else 0)
                fields = [fields[0], flag, fields[5], start+1, fields[11], cigar,
                          '*', 0, 0, '*', '*', 'NM:i:5']
            stream.write('\t'.join(map(str, fields)) + '\n')
    if bam:
        executable = shutil.which('samtools')
        if executable is None:
            raise RuntimeError('benchmark BAM fixture generation needs samtools')
        subprocess.run([executable, 'view', '-b', '-o', str(path), str(text_path)],
                       check=True, capture_output=True)
        text_path.unlink()
    return dict(alignments=reads*order, reads=reads, counts=counts, bytes=path.stat().st_size)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-python', required=True, type=Path)
    parser.add_argument('--candidate-python', type=Path, default=Path(__file__).resolve().parents[1] / 'python')
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--alignments', type=positive, default=80000)
    parser.add_argument('--mode', choices=['paf2pairs', 'paf2concat', 'bam2pairs', 'bam2concat'], default='paf2pairs')
    parser.add_argument('--repetitions', type=positive, default=3)
    parser.add_argument('--batch-rows', type=positive, default=65536)
    parser.add_argument('--chunk-size', type=positive, default=1000000)
    parser.add_argument('--layouts', nargs='+', choices=['low', 'high', 'scattered-gzip'],
                        default=['low', 'high', 'scattered-gzip'])
    parser.add_argument('--profile', action='store_true', help='profile each run; do not compare profiled timings to unprofiled timings')
    args = parser.parse_args()
    versions = dict(baseline=args.baseline_python.resolve(), candidate=args.candidate_python.resolve())
    for path in versions.values():
        if not (path / 'pqsio/import_alignments.py').is_file():
            parser.error(f'importer not found below {path}')
    native = Path(os.environ['PQSIO_LIBRARY'])
    cpu_model = platform.processor()
    cpu_info = Path('/proc/cpuinfo')
    if cpu_info.exists():
        with cpu_info.open() as stream:
            cpu_model = next((line.split(':', 1)[1].strip() for line in stream
                              if line.startswith('model name')), cpu_model)
    report = dict(settings={key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
                  platform=platform.platform(), cpu_model=cpu_model, python=sys.version, native=str(native),
                  timestamp_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
                  benchmark_sha256=sha256(Path(__file__).resolve()),
                  native_sha256=sha256(native),
                  rust_source_sha256={str(path.relative_to(Path(__file__).resolve().parents[1])): sha256(path)
                                      for path in [Path(__file__).resolve().parents[1] / 'src/import.rs',
                                                   Path(__file__).resolve().parents[1] / 'src/io.rs',
                                                   *sorted((Path(__file__).resolve().parents[1] / 'src/import').glob('*.rs'))]},
                  source_sha256={name: sha256(path / 'pqsio/import_alignments.py')
                                 for name, path in versions.items()},
                  cpus=sorted(os.sched_getaffinity(0))[:4] if hasattr(os, 'sched_getaffinity') else None,
                  fixtures={}, runs=[], summary=[])
    root = Path(__file__).resolve().parents[1] / 'tests/output'
    root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='import-bench-', dir=root) as directory:
        directory = Path(directory)
        for layout in args.layouts:
            source = directory / (layout + ('.bam' if args.mode.startswith('bam') else '.paf'))
            expected = report['fixtures'][layout] = fixture(source, layout, args.alignments, args.mode)
            reference = None
            for repeat in range(args.repetitions):
                labels = ('baseline', 'candidate') if repeat % 2 == 0 else ('candidate', 'baseline')
                for label in labels:
                    with tempfile.TemporaryDirectory(dir=directory) as target:
                        cmd = [sys.executable, str(Path(__file__).resolve()), '--worker',
                               '--mode', args.mode, '--source', str(source), '--output', str(Path(target) / 'pairs'),
                               '--batch-rows', str(args.batch_rows), '--chunk-size', str(args.chunk_size)]
                        if args.profile:
                            cmd.append('--profile')
                        env = dict(os.environ, PYTHONPATH=str(versions[label]),
                                   POLARS_MAX_THREADS='4', RAYON_NUM_THREADS='4')
                        completed = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=300)
                        if completed.returncode:
                            raise RuntimeError(completed.stderr or completed.stdout)
                        run = json.loads(completed.stdout)
                    assert run['result']['counts'] == expected['counts'], (layout, run)
                    if reference is None:
                        reference = run['verification']
                        expected['reference'] = reference
                        expected['result'] = run['result']
                    assert run['verification'] == reference, (layout, label, 'semantic mismatch')
                    assert run['result'] == expected['result'], (layout, label, 'report mismatch')
                    run.pop('verification')
                    run.update(layout=layout, version=label, repeat=repeat)
                    report['runs'].append(run)
                    print(f"{layout} {label} #{repeat+1}: {run['seconds']:.3f}s, {run['peak_mib']:.1f} MiB, stages={run['stages']}", flush=True)
            medians = {label: statistics.median(run['seconds'] for run in report['runs']
                                              if run['layout'] == layout and run['version'] == label)
                       for label in versions}
            peaks = {label: statistics.median(run['peak_mib'] for run in report['runs']
                                            if run['layout'] == layout and run['version'] == label)
                     for label in versions}
            report['summary'].append(dict(layout=layout, seconds=medians, peak_mib=peaks,
                                          records_per_second={label: expected['counts']['q0_records']/seconds
                                                            for label, seconds in medians.items()},
                                          speedup=medians['baseline']/medians['candidate']))
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['summary'], indent=2))


if __name__ == '__main__':
    if '--worker' in sys.argv:
        parser = argparse.ArgumentParser()
        parser.add_argument('--worker', action='store_true')
        parser.add_argument('--mode', required=True)
        parser.add_argument('--source', type=Path, required=True)
        parser.add_argument('--output', type=Path, required=True)
        parser.add_argument('--batch-rows', type=positive, required=True)
        parser.add_argument('--chunk-size', type=positive, required=True)
        parser.add_argument('--profile', action='store_true')
        worker(parser.parse_args())
    else:
        main()
