#!/usr/bin/env python3
"""Compare real-data pairs/concat text, gzip and PQS compression settings.

Run through Pixi. Source datasets are read-only; temporary fixtures are removed.
Only JSON measurements and a Markdown table remain below tests/output.
"""
import argparse
import gzip
import hashlib
import itertools
import json
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).resolve()
VARIANTS = {
    'text': dict(label='Text', compression=None, level=None),
    'gzip': dict(label='Gzip 6', compression='gzip', level=6),
    'pqs_uncompressed': dict(label='PQS uncompressed', compression='uncompressed', level=None),
    'pqs_default': dict(label='PQS default', compression='zstd', level=None),
}
FORMATS = tuple(VARIANTS)
OPERATIONS = ('read_all', 'mapq_count')


def is_pqs(storage):
    return storage == 'pqs' or storage.startswith('pqs_')


def digest_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def schema(kind):
    import polars as pl
    if kind == 'pairs':
        return dict(read_idx=pl.String, chrom1=pl.String, pos1=pl.UInt64,
                    chrom2=pl.String, pos2=pl.UInt64, strand1=pl.String,
                    strand2=pl.String, mapq=pl.UInt8)
    return dict(read_idx=pl.UInt64, read_length=pl.UInt32, read_start=pl.UInt32,
                read_end=pl.UInt32, strand=pl.String, chrom=pl.String,
                start=pl.UInt64, end=pl.UInt64, mapping_quality=pl.UInt8,
                identity=pl.Float32, filter_reason=pl.String)


def read_frame(path, kind, storage, operation):
    import polars as pl
    fields = schema(kind)
    quality = 'mapq' if kind == 'pairs' else 'mapping_quality'
    columns = list(fields) if operation == 'read_all' else [quality]
    if is_pqs(storage):
        # q1 is the format's stored MAPQ >= 1 subset. It is included in disk size.
        partition = path / ('q0' if operation == 'read_all' else 'q1')
        files = sorted(partition.glob('*.parquet'), key=lambda p: int(p.stem))
        frame = (pl.read_parquet(files, columns=columns) if files else
                 pl.DataFrame(schema={name: fields[name] for name in columns}))
    else:
        # Eager readers for all formats: this Polars version cannot scan gzip CSV.
        frame = pl.read_csv(path, has_header=False, separator='\t',
                            comment_prefix='#', quote_char=None, schema=fields,
                            columns=[list(fields).index(name) for name in columns])
    # Produce identical logical output types, including decoded chromosome names.
    return frame.with_columns([pl.col(name).cast(fields[name]) for name in columns])


def worker(config):
    os.sched_setaffinity(0, config['cpus'])
    import polars as pl
    assert pl.thread_pool_size() == config['threads']
    path = Path(config['path'])
    started = time.perf_counter()
    frame = read_frame(path, config['kind'], config['storage'], config['operation'])
    if config['operation'] == 'mapq_count':
        quality = 'mapq' if config['kind'] == 'pairs' else 'mapping_quality'
        count = frame.select((pl.col(quality) >= 30).sum()).item()
    else:
        count = frame.height
    elapsed = time.perf_counter() - started
    peak = int(next(line.split()[1] for line in Path('/proc/self/status').read_text().splitlines()
                    if line.startswith('VmHWM:'))) / 1024
    # Verification is deliberately after both wall-time and peak-RSS measurement.
    result = dict(seconds=elapsed, peak_rss_mib=peak, records=count)
    if config['operation'] == 'read_all':
        result['semantic_sha256'] = hashlib.sha256(frame.write_csv().encode()).hexdigest()
    print(json.dumps(result))


def sample_dataset(source, output, kind, requested):
    import pqsio
    if kind == 'concat':
        return sample_concat(source, output, requested)
    written = 0
    with pqsio.StreamingReader(source, batch_rows=65536) as reader:
        if reader.kind != kind:
            raise ValueError(f'{source}: expected {kind}, found {reader.kind}')
        with pqsio.PairsWriter(output, reader.contigs, chunk_size=100000) as writer:
            for batch in reader.iter_batches():
                take = min(len(batch), requested - written)
                rows = batch[:take]
                writer.write_batch(rows)
                written += len(rows)
                if written >= requested:
                    break
    if written < requested:
        raise ValueError(f'{source}: requested {requested} rows but found only {written}')
    report = pqsio.validate(output, level='full')
    if report.status != 'valid':
        raise ValueError(f'Generated sample failed validation: {report.to_dict()}')
    return written


def sample_concat(source, output, requested):
    """Normalize historical nonmonotonic, complete read groups."""
    import polars as pl
    import pqsio
    metadata = pqsio.inspect(source).metadata.to_dict()
    if metadata.get('format') != 'concat' or metadata.get('read_idx_scope') != 'global':
        raise ValueError('Concat sampling requires a global-ID concat source')
    paths = sorted((source / 'q0').glob('*.parquet'), key=lambda p: int(p.stem))
    limit = requested + 1024
    while True:
        frame = pl.read_parquet(paths, n_rows=limit).select([
            pl.col(name).cast(dtype) for name, dtype in schema('concat').items()])
        if frame.height < requested:
            raise ValueError(f'{source}: fewer than {requested} alignments')
        stop = requested
        ids = frame['read_idx']
        while stop < frame.height and ids[stop] == ids[stop - 1]:
            stop += 1
        if stop < frame.height or frame.height < limit:
            break
        limit += 65536
    frame = frame.head(stop)
    groups = frame['read_idx'].filter(
        frame['read_idx'] != frame['read_idx'].shift(1).fill_null(2**64 - 1))
    if groups.n_unique() != len(groups):
        raise ValueError('Source prefix has noncontiguous read groups; cannot infer complete reads')
    # Sorting changes order only. Read IDs and every alignment field are preserved.
    frame = frame.sort('read_idx', maintain_order=True)
    contigs = [(name, int(length)) for name, length in
               (line.split() for line in (source / '_contigsizes').read_text().splitlines())]
    contig_ids = {name: index for index, (name, _) in enumerate(contigs)}
    with pqsio.ConcatWriter(output, contigs, chunk_size=100000) as writer:
        reads, count = [], 0
        for _, rows in itertools.groupby(frame.iter_rows(), key=lambda row: row[0]):
            read = [pqsio.Alignment(*row[:5], contig_ids[row[5]], *row[6:]) for row in rows]
            reads.append(read)
            count += len(read)
            if count >= 8192:
                writer.write_reads(reads)
                reads, count = [], 0
        if reads:
            writer.write_reads(reads)
    report = pqsio.validate(output, level='full')
    if report.status != 'valid':
        raise ValueError(f'Generated concat sample failed validation: {report.to_dict()}')
    return stop


def write_text(frame, contigs, kind, path, compressed):
    def serialize(stream):
        if kind == 'pairs':
            header = '## pairs format v1.0\n#shape: whole matrix\n'
            header += ''.join(f'#chromsize: {name} {length}\n' for name, length in contigs)
            header += '#columns: readID chrom1 pos1 chrom2 pos2 strand1 strand2 mapq\n'
            stream.write(header.encode())
        frame.write_csv(stream, separator='\t', include_header=False, quote_style='never')

    with path.open('wb') as raw:
        if compressed:
            with gzip.GzipFile(filename='', mode='wb', fileobj=raw,
                               mtime=0, compresslevel=6) as stream:
                serialize(stream)
        else:
            serialize(raw)


def write_variant(path, storage, kind, frame, contigs, columns):
    import pqsio
    if is_pqs(storage):
        variant = VARIANTS[storage]
        cls = pqsio.PairsWriter if kind == 'pairs' else pqsio.ConcatWriter
        with cls(path, contigs, chunk_size=100000, compression=variant['compression'],
                 compression_level=variant['level']) as writer:
            for batch in columns:
                writer.write_columns(batch)
    else:
        write_text(frame, contigs, kind, path, compressed=storage == 'gzip')


def fixture(root, source, kind, requested):
    import polars as pl
    import pqsio
    sample = root / 'input.pqs'
    actual = sample_dataset(source, sample, kind, requested)
    frame = read_frame(sample, kind, 'pqs', 'read_all')
    quality = 'mapq' if kind == 'pairs' else 'mapping_quality'
    expected_q1 = hashlib.sha256(frame.filter(pl.col(quality) >= 1).write_csv().encode()).hexdigest()
    with pqsio.Reader(sample) as reader:
        contigs = list(reader.contigs)
        columns = list(reader.iter_columns())
    paths = {storage: root / storage for storage in FORMATS}
    for storage, path in paths.items():
        write_variant(path, storage, kind, frame, contigs, columns)
        if is_pqs(storage):
            if pqsio.validate(path, level='full').status != 'valid':
                raise ValueError(f'Invalid PQS fixture: {storage}')
            files = sorted((path / 'q1').glob('*.parquet'), key=lambda p: int(p.stem))
            fields = schema(kind)
            q1 = (pl.read_parquet(files).select([pl.col(n).cast(t) for n, t in fields.items()])
                  if files else pl.DataFrame(schema=fields))
            if hashlib.sha256(q1.write_csv().encode()).hexdigest() != expected_q1:
                raise ValueError(f'PQS q1 contents differ: {storage}')
    # These are freshly generated fixtures, never the large original source tree.
    files = {storage: ([p for p in path.iterdir() if p.is_file()]
                      + list((path / 'q0').glob('*.parquet'))
                      + list((path / 'q1').glob('*.parquet')) if path.is_dir() else [path])
             for storage, path in paths.items()}
    files = {storage: [path for path in items if path.is_file()] for storage, items in files.items()}
    sizes = {storage: sum(path.stat().st_size for path in items) for storage, items in files.items()}
    return paths, files, dict(requested_rows=requested, rows=actual, bytes=sizes,
                             counts=pqsio.info(sample).to_dict(), q1_record_sha256=expected_q1)


def warm(files):
    for path in files:
        with path.open('rb') as stream:
            while stream.read(1024 * 1024):
                pass


def run(config, files, environment):
    warm(files)
    result = subprocess.run([sys.executable, str(SCRIPT), '--worker', json.dumps(config)],
                            env=environment, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def source_description(path):
    return dict(path=str(path), metadata_sha256={name: digest_file(path / name)
                for name in ('_metadata', '_metadata_counts', '_contigsizes')})


def write_report(path, report):
    path.write_text(json.dumps(report, indent=2) + '\n')
    lines = ['| Kind | Rows | Format | Size (MiB) | Read all (ms) | MAPQ ≥ 30 count (ms) | Read peak RSS (MiB) |',
             '| --- | ---: | --- | ---: | ---: | ---: | ---: |']
    for case in report['cases']:
        for storage in FORMATS:
            read = case['summary']['read_all'][storage]
            count = case['summary']['mapq_count'][storage]
            label = VARIANTS[storage]['label']
            lines.append(f"| {case['kind']} | {case['rows']:,} | {label} | "
                         f"{case['bytes'][storage] / 2**20:.2f} | {read['median_seconds'] * 1000:.2f} | "
                         f"{count['median_seconds'] * 1000:.2f} | {read['median_peak_rss_mib']:.1f} |")
    path.with_suffix('.md').write_text('\n'.join(lines) + '\n')


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
        parser.error('--sizes must each be in 1..1000000 for this bounded benchmark')
    cpus = sorted(os.sched_getaffinity(0))[:args.threads]
    if args.repetitions < 1 or args.threads < 1 or len(cpus) < args.threads:
        parser.error('require positive repetitions/threads and enough available CPUs')
    output_root = (ROOT / 'tests/output').resolve()
    report_path = args.report.resolve()
    if not report_path.is_relative_to(output_root):
        parser.error('--report must be below tests/output')
    if report_path.exists() or report_path.with_suffix('.md').exists():
        parser.error('report outputs already exist; choose a new --report path')
    report_path.parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment.update(POLARS_MAX_THREADS=str(args.threads), RAYON_NUM_THREADS=str(args.threads),
                       OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1')
    import polars as pl
    import pqsio
    sources = dict(pairs=args.pairs_input.resolve(), concat=args.concat_input.resolve())
    cpu = next(line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').open()
               if line.startswith('model name'))
    report = dict(benchmark_version=2, variants=VARIANTS,
                  date=time.strftime('%Y-%m-%d %H:%M:%S %z'), cpu=cpu,
                  platform=platform.platform(), python=platform.python_version(),
                  polars=pl.__version__, pqsio=pqsio.__version__, cpus=cpus,
                  threads=args.threads, repetitions=args.repetitions, warmups=1,
                  load_start=os.getloadavg(), script_sha256=digest_file(SCRIPT),
                  native_library_sha256=digest_file(Path(os.environ['PQSIO_LIBRARY'])),
                  sources={kind: source_description(path) for kind, path in sources.items()},
                  sampling='Nested prefixes of q0; concat rounded up to a complete read and '
                           'stable-sorted by original read_idx to normalize historical ID ordering',
                  method='Same Polars eager readers; full read includes logical type normalization; '
                         'MAPQ count projects the quality column and reads PQS q1; gzip level 6; '
                         'all fixtures use the same prepared columns and serialization as the write test; '
                         'warm OS cache requested; fresh pinned worker per sample; imports, preparation '
                         'and verification excluded from timer; VmHWM measured before verification; '
                         'PQS size includes both quality partitions and all metadata; no index',
                  cases=[])
    rng = random.Random(20260920)
    for kind, source in sources.items():
        for requested in sorted(set(args.sizes)):
            print(f'Preparing {kind}: {requested:,} rows', flush=True)
            with tempfile.TemporaryDirectory(prefix='format-bench-', dir=output_root) as temporary:
                paths, files, case = fixture(Path(temporary), source, kind, requested)
                case.update(kind=kind, samples={op: {f: [] for f in FORMATS} for op in OPERATIONS})
                expected = {}
                for repetition in range(-1, args.repetitions):
                    jobs = [(op, storage) for op in OPERATIONS for storage in FORMATS]
                    rng.shuffle(jobs)
                    for operation, storage in jobs:
                        config = dict(kind=kind, path=str(paths[storage]), storage=storage,
                                      operation=operation, cpus=cpus, threads=args.threads)
                        value = run(config, files[storage], environment)
                        signature = (value['records'], value.get('semantic_sha256'))
                        if operation not in expected:
                            expected[operation] = signature
                        if signature != expected[operation]:
                            raise ValueError(f'Result mismatch for {kind}/{operation}/{storage}')
                        if operation == 'read_all' and value['records'] != case['rows']:
                            raise ValueError('Read did not consume the complete fixture')
                        if repetition >= 0:
                            case['samples'][operation][storage].append(value)
                    print(f"  {kind} {case['rows']:,}: {'warmup' if repetition == -1 else f'run {repetition + 1}'} verified", flush=True)
                case['verification'] = dict(full_record_sha256=expected['read_all'][1],
                                            mapq30_records=expected['mapq_count'][0],
                                            all_formats_equal=True)
                case['summary'] = {}
                for operation in OPERATIONS:
                    case['summary'][operation] = {}
                    for storage, samples in case['samples'][operation].items():
                        times = [sample['seconds'] for sample in samples]
                        case['summary'][operation][storage] = dict(
                            median_seconds=statistics.median(times), min_seconds=min(times),
                            max_seconds=max(times), median_peak_rss_mib=statistics.median(
                                sample['peak_rss_mib'] for sample in samples))
                report['cases'].append(case)
                report['load_end'] = os.getloadavg()
                write_report(report_path, report)
                print(f"Verified {kind}: {case['rows']:,} rows, all formats identical", flush=True)
    print(f'Reports: {report_path} and {report_path.with_suffix(".md")}', flush=True)


if __name__ == '__main__':
    main()
