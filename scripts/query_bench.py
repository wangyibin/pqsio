"""Opt-in modest synthetic benchmark. Outputs JSON; no privileged cache flushing.

Each query runs in a fresh worker. The parent warms source/index files before
measurement. Worker wall time includes opening, identity/integrity checks and
consuming all column buffers; excludes fixture/build time. Linux VmHWM is whole
worker peak RSS since exec, including startup, not an allocation-byte limit.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import statistics
import subprocess
import sys
import tempfile
import time


def worker(path, mode):
    import pqsio
    started = time.perf_counter()
    digest = hashlib.sha256()
    with pqsio.QueryReader(path, [('chr1',50000,51000)], min_mapq=30,
                           index=mode, batch_rows=4096) as reader:
        for batch in reader.iter_columns():
            for field, _ in batch._schema:
                digest.update(memoryview(getattr(batch,field)).cast('B'))
        stats = reader.stats
    seconds = time.perf_counter()-started
    # getrusage can carry a parent RSS high-water floor through process creation;
    # /proc VmHWM measures this worker's current address space since exec.
    status = Path('/proc/self/status')
    if status.exists():
        peak = int(next(line.split()[1] for line in status.read_text().splitlines() if line.startswith('VmHWM:')))/1024
        source = 'Linux /proc/self/status VmHWM'
    else:
        peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss/1024
        source = 'ru_maxrss; may include inherited parent high-water floor'
    print(json.dumps(dict(seconds=seconds,peak_rss_mib=peak,peak_rss_source=source,
                          digest=digest.hexdigest(),stats=stats)))


def fixture(root, distribution, rows, group):
    import polars as pl
    import pqsio
    import random
    rng = random.Random(173)
    path = root/distribution
    with pqsio.PairsWriter(path,{'chr1':1000000,'chr2':1000000},chunk_size=rows) as writer:
        # Moderate named fixture only, generated in batches.
        for base in range(0,rows,group):
            batch=[]
            for i in range(base,min(base+group,rows)):
                block=i//group if distribution=='clustered' else rng.randrange(rows//group)
                pos=block*10000+rng.randrange(1000)+1
                batch.append(pqsio.Pair(str(i),0,pos,1,pos,'+','-',60 if i%4 else 0))
            writer.write_batch(batch)
    for quality in ('q0','q1'):
        for shard in (path/quality).glob('*.parquet'):
            frame=pl.read_parquet(shard)
            frame.write_parquet(shard,row_group_size=group,statistics=False)
    return path


def warm(path):
    files=[path/'_metadata',path/'_contigsizes',*sorted((path/'q1').glob('*.parquet'))]
    root=path/'.pqsio-index'/'q1';generation=root/(root/'CURRENT').read_text()
    files.extend([root/'CURRENT',generation/'manifest.json',*generation.glob('*.rg')])
    for file in files:
        with file.open('rb') as source:
            while source.read(65536):pass


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--worker',nargs=2,metavar=('PATH','MODE'))
    parser.add_argument('--rows',type=int,default=32768)
    parser.add_argument('--group',type=int,default=1024)
    parser.add_argument('--repetitions',type=int,default=5)
    args=parser.parse_args()
    if args.worker:
        worker(*args.worker);return
    if not (args.rows>=6*args.group>0 and args.rows%args.group==0 and args.rows//args.group<=100 and args.repetitions>0):
        parser.error('require 6..100 full row groups and positive repetitions')
    import pqsio
    root=Path(__file__).resolve().parents[1]/'tests/output'
    root.mkdir(exist_ok=True)
    report={'conditions':'warm OS page cache requested by parent reads; fresh worker and zero index cache each trial; no cold-cache claim',
            'source_quality':'q1','rows':args.rows,'row_group_target':args.group,'repetitions':args.repetitions,'results':[]}
    with tempfile.TemporaryDirectory(prefix='query-bench-',dir=root) as temp:
        for distribution in ('clustered','mixed'):
            started=time.perf_counter();path=fixture(Path(temp),distribution,args.rows,args.group)
            fixture_seconds=time.perf_counter()-started
            started=time.perf_counter();pqsio.build_index(path,quality="q1");build_seconds=time.perf_counter()-started
            trials={'off':[],'require':[]}
            for repeat in range(args.repetitions):
                for mode in (('off','require') if repeat%2==0 else ('require','off')):
                    warm(path)
                    result=subprocess.run([sys.executable,__file__,'--worker',str(path),mode],capture_output=True,text=True,check=True)
                    trials[mode].append(json.loads(result.stdout))
            digests={t['digest'] for group in trials.values() for t in group}
            assert len(digests)==1,'indexed/sequential output mismatch'
            summary={}
            for mode,values in trials.items():
                summary[mode]={'median_seconds':statistics.median(v['seconds'] for v in values),
                               'median_peak_rss_mib':statistics.median(v['peak_rss_mib'] for v in values),
                               'max_peak_rss_mib':max(v['peak_rss_mib'] for v in values),
                               'stats':values[0]['stats'],'trials':values}
            report['results'].append(dict(distribution=distribution,fixture_seconds=fixture_seconds,
                                           index_build_seconds=build_seconds,equal=True,measurements=summary))
    print(json.dumps(report,indent=2))


if __name__=='__main__':main()
