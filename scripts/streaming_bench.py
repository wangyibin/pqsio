"""Reproducible modest synthetic streaming measurement (Linux /proc/self/status VmHWM).
Fixture construction is in the parent; each read is a fresh subprocess.
Run with Pixi from pqsio: python scripts/streaming_bench.py
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def consume(path, boundary, filtering):
    import pqsio
    start = time.perf_counter()
    count = batches = largest = 0
    with pqsio.StreamingReader(path, 30, batch_rows=1024,
                              boundary=boundary, filter_mode=filtering) as reader:
        for batch in reader.iter_batches():
            count += len(batch)
            batches += 1
            largest = max(largest, len(batch))
            del batch
    elapsed = time.perf_counter() - start
    peak_kib = int(next(line.split()[1] for line in Path("/proc/self/status").read_text().splitlines() if line.startswith("VmHWM:")))
    print(json.dumps(dict(rows=count, batches=batches, largest_batch=largest,
                         seconds=round(elapsed, 4), rows_per_second=round(count/elapsed),
                         peak_mib=round(peak_kib/1024, 2))))


def main():
    import polars as pl
    import pqsio as p
    root = Path(__file__).resolve().parents[1] / 'tests' / 'output'
    root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='streaming-measure-', dir=root) as tmp:
        results = []
        for layout in ('ordinary', 'single_read'):
            for n in (20000, 80000, 320000):
                path = Path(tmp) / f'{layout}-{n}'
                with p.ConcatWriter(path, {'chr1': 1000}) as writer:
                    writer.write_read([p.Alignment(1,100,0,50,'+',0,0,50,60,.5)])
                shard = next((path/'q0').glob('*.parquet'))
                seed = pl.read_parquet(shard)
                frame = pl.DataFrame({
                    'read_idx': pl.Series([i//4+1 if layout=='ordinary' else 1 for i in range(n)], dtype=pl.UInt64),
                    'read_length': pl.Series([100]*n,dtype=pl.UInt32),
                    'read_start': pl.Series([0]*n,dtype=pl.UInt32),
                    'read_end': pl.Series([50]*n,dtype=pl.UInt32),
                    'strand': ['+']*n, 'chrom': ['chr1']*n,
                    'start': pl.Series([0]*n,dtype=pl.UInt64),
                    'end': pl.Series([50]*n,dtype=pl.UInt64),
                    'mapping_quality': pl.Series([60]*n,dtype=pl.UInt8),
                    'identity': pl.Series([.5]*n,dtype=pl.Float32),
                    'filter_reason': ['pass']*n,
                }).select([pl.col(c).cast(t) for c,t in seed.schema.items()])
                frame.write_parquet(shard, row_group_size=4096)
                frame.write_parquet(path/'q1'/shard.name, row_group_size=4096)
                del frame, seed
                for boundary, filtering in [('rows','matching_alignments'),('complete_reads','complete_reads')]:
                    for repetition in range(2):
                        output=subprocess.check_output([sys.executable,__file__,'--consume',str(path),boundary,filtering],text=True)
                        result=dict(layout=layout,shard_rows=n,row_group_target=4096,batch_rows=1024,
                                    boundary=boundary,filtering=filtering,repetition=repetition,**json.loads(output))
                        results.append(result)
                        print(json.dumps(result),flush=True)
        output=Path(os.environ.get('PQSIO_MEASURE_OUT', str(root/'streaming-measurements.json')))
        output.write_text(json.dumps(results,indent=2)+'\n')

if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--consume': consume(*sys.argv[2:])
    else: main()
