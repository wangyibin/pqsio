#!/usr/bin/env python3
"""Measure complete PQS, text and gzip writes from prepared column data.

Run with Pixi. Input datasets are read-only. Bounded real-data subsets and
temporary outputs stay under tests/output and are removed after each case.
PQS uses native write_columns; text and gzip use Polars TSV serialization.
Preparation of each writer's equivalent input representation is not timed.
"""
import argparse
import gc
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import format_bench as read_bench


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).resolve()
FORMATS = read_bench.FORMATS
OPERATION = 'write_all'


def rss_mib(field):
    return int(next(line.split()[1] for line in Path('/proc/self/status').read_text().splitlines()
                    if line.startswith(field + ':'))) / 1024


def semantic_hash(frame):
    return hashlib.sha256(frame.write_csv().encode()).hexdigest()


def output_bytes(path):
    if path.is_file():
        return path.stat().st_size
    files = [p for p in path.iterdir() if p.is_file()]
    for quality in ('q0', 'q1'):
        files.extend((path / quality).glob('*.parquet'))
    return sum(p.stat().st_size for p in files)


def worker(config):
    os.sched_setaffinity(0, config['cpus'])
    import polars as pl
    import pqsio
    if pl.thread_pool_size() != config['threads']:
        raise ValueError('Polars thread pool does not match the CPU budget')
    source, path = Path(config['source']), Path(config['output'])
    kind, storage = config['kind'], config['storage']

    # Prepare BOTH input representations in every worker, including identical
    # allocations in the process RSS baseline. Concat batches are complete reads.
    frame = read_bench.read_frame(source, kind, 'pqs', 'read_all')
    with pqsio.Reader(source) as reader:
        contigs = list(reader.contigs)
        columns = list(reader.iter_columns())
    if sum(len(batch) for batch in columns) != frame.height:
        raise ValueError('Prepared row counts differ')
    gc.collect()
    baseline_rss = rss_mib('VmRSS')
    started = time.perf_counter()
    read_bench.write_variant(path, storage, kind, frame, contigs, columns)
    elapsed = time.perf_counter() - started
    peak_rss = rss_mib('VmHWM')

    # Read-back, validation, checksums and deletion do not contribute to time/RSS.
    result = dict(seconds=elapsed, peak_rss_mib=peak_rss,
                  baseline_rss_mib=baseline_rss, bytes=output_bytes(path))
    if read_bench.is_pqs(storage):
        validation = pqsio.validate(path, level='full')
        if validation.status != 'valid':
            raise ValueError(f'Written PQS failed validation: {validation.to_dict()}')
    decoded = read_bench.read_frame(path, kind, storage, 'read_all')
    result['records'] = decoded.height
    result['semantic_sha256'] = semantic_hash(decoded)
    quality = 'mapq' if kind == 'pairs' else 'mapping_quality'
    if read_bench.is_pqs(storage):
        paths = sorted((path / 'q1').glob('*.parquet'), key=lambda p: int(p.stem))
        fields = read_bench.schema(kind)
        q1 = (pl.read_parquet(paths).select([pl.col(n).cast(t) for n, t in fields.items()])
              if paths else pl.DataFrame(schema=fields))
    else:
        q1 = decoded.filter(pl.col(quality) >= 1)
    result['q1_records'] = q1.height
    result['q1_semantic_sha256'] = semantic_hash(q1)
    if result['semantic_sha256'] != config['expected_sha256']:
        raise ValueError('Written records differ from the prepared input')
    if result['q1_semantic_sha256'] != config['expected_q1_sha256']:
        raise ValueError('Written quality subset differs from the prepared input')
    print(json.dumps(result))


def write_report(path, report):
    path.write_text(json.dumps(report, indent=2) + '\n')
    rows = ['| Dataset | Records | Variant | Write (ms) | Size (MiB) |',
            '| --- | ---: | --- | ---: | ---: |']
    for case in report['cases']:
        for storage, stats in case['summary'][OPERATION].items():
            rows.append(f"| {case['kind']} | {case['rows']:,} | {read_bench.VARIANTS[storage]['label']} | "
                        f"{stats['median_seconds'] * 1000:.2f} | {case['bytes'][storage] / 2**20:.2f} |")
    path.with_suffix('.md').write_text('\n'.join(rows) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', help=argparse.SUPPRESS)
    parser.add_argument('--pairs-input', type=Path)
    parser.add_argument('--concat-input', type=Path)
    parser.add_argument('--sizes', type=int, nargs='+', default=[10000, 100000, 1000000])
    parser.add_argument('--threads', type=int, default=4)
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    if args.worker:
        worker(json.loads(args.worker))
        return
    if not args.pairs_input or not args.concat_input or not args.report:
        parser.error('--pairs-input, --concat-input and --report are required')
    if any(not 1 <= size <= 1000000 for size in args.sizes):
        parser.error('--sizes must be in 1..1000000 for this bounded benchmark')
    cpus = sorted(os.sched_getaffinity(0))[:args.threads]
    if args.repetitions < 1 or args.threads < 1 or len(cpus) < args.threads:
        parser.error('require positive repetitions/threads and enough available CPUs')
    output_root = (ROOT / 'tests/output').resolve()
    report_path = args.report.resolve()
    if not report_path.is_relative_to(output_root):
        parser.error('--report must be below tests/output')
    if report_path.exists() or report_path.with_suffix('.md').exists():
        parser.error('report already exists; choose a new --report path')
    report_path.parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment.update(POLARS_MAX_THREADS=str(args.threads), RAYON_NUM_THREADS=str(args.threads),
                       OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1')
    import polars as pl
    import pqsio
    sources = dict(pairs=args.pairs_input.resolve(), concat=args.concat_input.resolve())
    cpu = next(line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').open()
               if line.startswith('model name'))
    library = Path(os.environ['PQSIO_LIBRARY'])
    report = dict(benchmark_version=2, variants=read_bench.VARIANTS, date=time.strftime('%Y-%m-%d %H:%M:%S %z'), cpu=cpu,
                  platform=platform.platform(), python=platform.python_version(),
                  polars=pl.__version__, pqsio=pqsio.__version__, native_polars='0.49.1',
                  cpus=cpus, threads=args.threads, repetitions=args.repetitions, warmups=1,
                  script_sha256=read_bench.digest_file(SCRIPT),
                  sampling_script_sha256=read_bench.digest_file(Path(read_bench.__file__)),
                  native_library_sha256=read_bench.digest_file(library),
                  sources={k: read_bench.source_description(p) for k, p in sources.items()},
                  method='Prepared native column buffers and a logically equivalent Polars frame '
                         'are retained in every worker. PQS native synchronous write_columns, '
                         'uncompressed or default Zstd, 100000-row shard target, q0+q1+metadata. '
                         'Text: Polars TSV; gzip: same '
                         'TSV serialization into ordinary single-stream gzip level 6. Timer '
                         'includes open, serialization, compression, close and PQS finish; '
                         'excludes input loading/adaptation, imports, validation and read-back. '
                         'No fsync; buffered filesystem writes. VmHWM before read-back includes '
                         'input preparation and both input representations. Fresh CPU-pinned '
                         'workers, shuffled formats, one warmup and measured repeats.',
                  load_start=os.getloadavg(), cases=[])
    rng = random.Random(20260920)
    for kind, source in sources.items():
        for requested in sorted(set(args.sizes)):
            print(f'Preparing {kind}: {requested:,} records', flush=True)
            with tempfile.TemporaryDirectory(prefix='format-write-bench-', dir=output_root) as tmp:
                base = Path(tmp)
                fixture = base / f'input.{kind}.pqs'
                actual = read_bench.sample_dataset(source, fixture, kind, requested)
                frame = read_bench.read_frame(fixture, kind, 'pqs', 'read_all')
                quality = 'mapq' if kind == 'pairs' else 'mapping_quality'
                expected = semantic_hash(frame)
                expected_q1 = semantic_hash(frame.filter(pl.col(quality) >= 1))
                del frame
                case = dict(kind=kind, requested_rows=requested, rows=actual, bytes={},
                            samples={OPERATION: {s: [] for s in FORMATS}},
                            verification=dict(full_record_sha256=expected,
                                              q1_record_sha256=expected_q1, all_formats_equal=True))
                for repetition in range(-1, args.repetitions):
                    jobs = list(FORMATS)
                    rng.shuffle(jobs)
                    for storage in jobs:
                        output = base / ('result-' + storage)
                        config = dict(source=str(fixture), output=str(output), kind=kind,
                                      storage=storage, cpus=cpus, threads=args.threads,
                                      expected_sha256=expected, expected_q1_sha256=expected_q1)
                        process = subprocess.run([sys.executable, str(SCRIPT), '--worker', json.dumps(config)],
                                                 env=environment, text=True, capture_output=True, timeout=300)
                        if process.returncode:
                            raise RuntimeError(process.stderr)
                        result = json.loads(process.stdout)
                        if result['records'] != actual:
                            raise ValueError('Writer did not preserve all records')
                        previous_size = case['bytes'].setdefault(storage, result['bytes'])
                        if result['bytes'] != previous_size:
                            raise ValueError('Output size changed between repetitions')
                        if repetition >= 0:
                            case['samples'][OPERATION][storage].append(result)
                        if output.is_dir():
                            shutil.rmtree(output)
                        else:
                            output.unlink()
                    print(f"  {kind} {actual:,}: {'warmup' if repetition == -1 else f'run {repetition + 1}'} verified", flush=True)
                summary = {}
                for storage, samples in case['samples'][OPERATION].items():
                    times = [s['seconds'] for s in samples]
                    summary[storage] = dict(median_seconds=statistics.median(times),
                                            min_seconds=min(times), max_seconds=max(times),
                                            median_peak_rss_mib=statistics.median(s['peak_rss_mib'] for s in samples))
                case['summary'] = {OPERATION: summary}
                report['cases'].append(case)
                report['load_end'] = os.getloadavg()
                write_report(report_path, report)
    print(f'Report: {report_path}', flush=True)


if __name__ == '__main__':
    main()
