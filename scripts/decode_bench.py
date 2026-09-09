"""Compare reflective and fixed-layout decoding on identical synthetic datasets."""
from dataclasses import fields
import json
import os
from pathlib import Path
import platform
import statistics
import tempfile
import time
import pqsio as p


def reflective(row, cls):
    values = {}
    for field in fields(cls):
        value = getattr(row, field.name)
        if isinstance(value, bytes):
            value = value.decode('utf-8')
        elif field.name.startswith('strand'):
            value = chr(value)
        values[field.name] = value
    return cls(**values)


def main():
    current = p._decode
    cpus = sorted(os.sched_getaffinity(0))[:4]
    os.sched_setaffinity(0, cpus)
    p._library()  # Exclude first dynamic-library load from measurements.
    root = Path(__file__).resolve().parents[1] / 'tests' / 'output'
    root.mkdir(exist_ok=True)
    results = {}
    with tempfile.TemporaryDirectory(dir=root) as tmp:
        for kind in ('pairs', 'concat'):
            path = Path(tmp) / kind
            if kind == 'pairs':
                rows = [p.Pair(f'读段{i}', 0, i + 1, 0, i + 2, '+', '-', i % 61) for i in range(80000)]
                writer = p.PairsWriter
            else:
                rows = [p.Alignment(i // 8 + 1, 100, 0, 50, '+', 0, i, i + 50, i % 61, 0.5, '通过') for i in range(80000)]
                writer = p.ConcatWriter
            with writer(path, [('ctg', 1000000)], chunk_size=20000) as w:
                if kind == 'pairs':
                    w.write_batch(rows)
                else:
                    w.write_batch(rows, list(range(0, len(rows) + 1, 8)))
            samples = {'before': [], 'after': []}
            for rep in range(6):
                order = ('before', 'after') if rep % 2 == 0 else ('after', 'before')
                for name in order:
                    p._decode = reflective if name == 'before' else current
                    start = time.perf_counter()
                    with p.Reader(path) as r:
                        actual = [row for batch in r.iter_batches() for row in batch]
                    elapsed = time.perf_counter() - start
                    assert actual == rows
                    if rep:
                        samples[name].append(elapsed)
            results[kind] = {name: {'median_seconds': statistics.median(values), 'samples': values} for name, values in samples.items()}
    p._decode = current
    print(json.dumps(dict(python=platform.python_version(), platform=platform.platform(),
                          rows=80000, repetitions=5, warmups=1, cpus=cpus,
                          polars_threads=os.environ.get('POLARS_MAX_THREADS'),
                          rayon_threads=os.environ.get('RAYON_NUM_THREADS'),
                          results=results), indent=2))


if __name__ == '__main__':
    main()
