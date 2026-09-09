"""Small complete-read benchmark; each sample uses a fresh Python process."""
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import sys
import tempfile
import time


def consume(path):
    import pqsio as p
    p._library()
    digest = hashlib.sha256()
    start = time.perf_counter()
    count = 0
    with p.StreamingReader(path, 0, batch_rows=4096, boundary='complete_reads') as reader:
        for batch in reader.iter_columns():
            count += len(batch)
            for field, _ in batch._schema:
                digest.update(memoryview(getattr(batch, field)))
    return dict(seconds=time.perf_counter() - start, rows=count,
                peak_mib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024,
                digest=digest.hexdigest())


def main():
    import pqsio as p
    cpus = sorted(os.sched_getaffinity(0))[:4]
    os.sched_setaffinity(0, cpus)
    root = Path(__file__).resolve().parents[1] / 'tests' / 'output'
    root.mkdir(exist_ok=True)
    report = dict(python=platform.python_version(), cpus=cpus, rows=80000,
                  repetitions=5, warmups=1, batch_rows=4096,
                  threads={k: os.environ.get(k) for k in ('POLARS_MAX_THREADS', 'RAYON_NUM_THREADS')}, cases={})
    with tempfile.TemporaryDirectory(prefix='stream-copy-', dir=root) as tmp:
        for name, read_size in (('short_reads', 8), ('single_large_read', 80000)):
            path = Path(tmp) / name
            rows = [p.Alignment(i // read_size + 1, 100, 0, 50, '+' if i % 2 else '-',
                                0, i, i + 50, i % 61, .5, '通过') for i in range(80000)]
            with p.ConcatWriter(path, {'chr1': 1000000}, chunk_size=20000) as w:
                w.write_batch(rows, list(range(0, 80001, read_size)))
            samples = []
            for rep in range(6):
                result = json.loads(subprocess.check_output(
                    [sys.executable, __file__, '--consume', str(path)], text=True))
                assert result['rows'] == 80000
                if rep == 0:
                    expected = result['digest']
                assert result['digest'] == expected
                if rep:
                    samples.append(result)
            report['cases'][name] = dict(samples=samples,
                median_seconds=statistics.median(s['seconds'] for s in samples),
                median_peak_mib=statistics.median(s['peak_mib'] for s in samples))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--consume':
        print(json.dumps(consume(sys.argv[2])))
    else:
        main()
