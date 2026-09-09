"""Opt-in, bounded synthetic A/B conversion acceptance benchmark.

Run in Pixi. --baseline is a saved pre-change native library; --candidate
defaults to PQSIO_LIBRARY. Fresh subprocesses measure conversion wall time,
CPU time and peak RSS before correctness scans. No cache dropping or fsync.
Fixtures and transient outputs are kept under tests/output, then removed.
Only the JSON report is retained. Both libraries must support threads.
"""
import argparse
from array import array
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time


def fingerprint(path, quality):
    import pqsio as p
    names = ('read_id_bytes', 'id_lengths', 'chrom1', 'pos1', 'chrom2',
             'pos2', 'strand1', 'strand2', 'mapq')
    digests = {name: hashlib.sha256() for name in names}
    with p.Reader(path, quality) as reader:
        for batch in reader.iter_columns():
            for name in names:
                if name == 'id_lengths':
                    offsets = batch.read_id_offsets
                    data = array('Q', (b-a for a, b in zip(offsets, offsets[1:])))
                else:
                    data = getattr(batch, name)
                digests[name].update(bytes(data))
    return {name: value.hexdigest() for name, value in digests.items()}


def worker(source, output, threads, batch_rows, chunk_size):
    import pqsio as p
    p._library()  # Exclude dynamic library loading from timed conversion.
    start_cpu = time.process_time()
    start = time.perf_counter()
    result = p.convert(source, output, threads=threads, batch_rows=batch_rows,
                       chunk_size=chunk_size).to_dict()
    elapsed = time.perf_counter() - start
    cpu = time.process_time() - start_cpu
    peak = int(next(line.split()[1] for line in Path('/proc/self/status')
                    .read_text().splitlines() if line.startswith('VmHWM:'))) / 1024
    output = Path(output)
    # Correctness and filesystem size scans are outside time/RSS measurement.
    result.pop('output')
    print(json.dumps(dict(seconds=elapsed, cpu_seconds=cpu, peak_mib=peak,
        pairs_per_second=result['counts']['q0_records'] / elapsed,
        bytes=sum(f.stat().st_size for f in output.rglob('*') if f.is_file()),
        result=result, q0=fingerprint(output, 0), q1=fingerprint(output, 1),
        metadata={name: '\n'.join(line for line in (output/name).read_text().splitlines()
                                  if not line.strip().startswith("'creation_time':")) for name in
                  ('_metadata', '_metadata_counts', '_contigsizes', 'cn.info')})))


def fixture(path, orders, alignments):
    import pqsio as p
    offsets, ids, indices = [0], [], []
    q0 = q1 = 0
    while len(ids) < alignments:
        read_id = len(offsets)
        order = orders[(read_id-1) % len(orders)]
        ids.extend([read_id] * order)
        indices.extend(reversed(range(order)))
        offsets.append(len(ids))
        positive = sum(j % 5 != 0 for j in range(order))
        q0 += order*(order-1)//2
        q1 += positive*(positive-1)//2
    n = len(ids)
    so, data = p.pack_strings(['pass'] * n)
    columns = p.ConcatColumns(array('Q', offsets), array('Q', ids),
        array('I', [10000])*n, array('I', (j*10 for j in indices)),
        array('I', (j*10+5 for j in indices)),
        array('B', (43 if j % 2 else 45 for j in indices)),
        array('I', (j % 2 for j in indices)),
        array('Q', (2**33+j*100 for j in indices)),
        array('Q', (2**33+j*100+50 for j in indices)),
        array('B', (30 if j % 5 else 0 for j in indices)),
        array('f', [.9])*n, so, data)
    with p.ConcatWriter(path, {'a': 2**34, 'b': 2**34}, chunk_size=16384) as writer:
        writer.write_columns(columns)
    p.set_copy_numbers(path, {'a': 2})
    return dict(alignments=n, reads=len(offsets)-1, q0_records=q0, q1_records=q1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', required=True, type=Path)
    parser.add_argument('--candidate', type=Path, default=Path(os.environ['PQSIO_LIBRARY']))
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--alignments', type=int, default=80000)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--batch-rows', type=int, default=8192)
    parser.add_argument('--chunk-size', type=int, default=65536)
    parser.add_argument('--layouts', nargs='+', choices=['low', 'high', 'mixed'],
                        default=['low', 'high', 'mixed'])
    parser.add_argument('--threads', nargs='+', type=int, default=[1, 4])
    args = parser.parse_args()
    if min(args.alignments, args.repetitions, args.batch_rows, args.chunk_size, *args.threads) < 1:
        parser.error('sizes and repetitions must be positive')
    libraries = {'baseline': args.baseline.resolve(), 'candidate': args.candidate.resolve()}
    for library in libraries.values():
        if not library.is_file():
            parser.error(f'library missing: {library}')
    report = dict(settings=vars(args).copy(), platform=platform.platform(),
                  cpu_count=os.cpu_count(), libraries={k: hashlib.sha256(v.read_bytes()).hexdigest()
                    for k, v in libraries.items()}, fixtures={}, runs=[], summary=[])
    report['settings'] = {k: str(v) if isinstance(v, Path) else v for k, v in report['settings'].items()}
    root = Path(__file__).resolve().parents[1] / 'tests/output'
    root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='convert-bench-', dir=root) as tmp:
        tmp = Path(tmp)
        for layout, orders in [('low', [4]), ('high', [32]), ('mixed', [2, 4, 16, 64])]:
            if layout not in args.layouts:
                continue
            source = tmp / layout
            expected = report['fixtures'][layout] = fixture(source, orders, args.alignments)
            reference = None
            # Alternate baseline/candidate order to reduce simple cache-order bias.
            for repeat in range(args.repetitions):
                for threads in args.threads:
                    for label in (('baseline', 'candidate') if repeat % 2 == 0 else ('candidate', 'baseline')):
                        with tempfile.TemporaryDirectory(dir=tmp) as out:
                            env = dict(os.environ, PQSIO_LIBRARY=str(libraries[label]),
                                       POLARS_MAX_THREADS='4', RAYON_NUM_THREADS='4')
                            proc = subprocess.run([sys.executable, str(Path(__file__).resolve()),
                                '--worker', str(source), str(Path(out)/'pairs'), str(threads),
                                str(args.batch_rows), str(args.chunk_size)], env=env,
                                capture_output=True, text=True, check=True, timeout=120)
                            run = json.loads(proc.stdout)
                        assert run['result']['counts'] == {k: expected[k] for k in ('q0_records', 'q1_records')}
                        assert run['result']['output_reads'] == expected['reads']
                        signature = {k: run[k] for k in ('result', 'q0', 'q1', 'metadata')}
                        if reference is None:
                            reference = signature
                            report['fixtures'][layout]['reference'] = signature
                        assert signature == reference, (layout, label, threads,
                            [k for k in signature if signature[k] != reference[k]])
                        for key in ('q0', 'q1', 'metadata'):
                            run.pop(key)
                        run.update(layout=layout, library=label, threads=threads, repeat=repeat)
                        report['runs'].append(run)
                        print(f"{layout} {label} t={threads}: {run['seconds']:.3f}s, {run['peak_mib']:.1f} MiB", flush=True)
            for threads in args.threads:
                stats = {}
                for label in libraries:
                    runs = [r for r in report['runs'] if (r['layout'], r['threads'], r['library']) == (layout, threads, label)]
                    stats[label] = {key: statistics.median(r[key] for r in runs)
                                    for key in ('seconds', 'pairs_per_second', 'peak_mib', 'bytes', 'cpu_seconds')}
                report['summary'].append(dict(layout=layout, threads=threads, **stats,
                    speedup=stats['baseline']['seconds']/stats['candidate']['seconds']))
    report['correctness'] = 'All ordered q0/q1 column hashes, metadata (excluding creation_time) and analytical counts matched.'
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['summary'], indent=2))


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--worker':
        worker(sys.argv[2], sys.argv[3], *map(int, sys.argv[4:7]))
    else:
        main()
