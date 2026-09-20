#!/usr/bin/env python3
"""Small opt-in native CLI vs fresh-process Python API benchmark (Linux).

Run with Pixi. Fixtures stay in a repository-local TemporaryDirectory; only
measurements/report survive. No cache dropping, global affinity or source edits.
"""
import argparse
import gzip
import hashlib
import inspect
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def worker(config):
    import pqsio as p
    op, source, output, threads = config['op'], config['source'], config['output'], config['threads']
    if op == 'version':
        print(p.__version__)
        return
    if op == 'info': result = p.info(source)
    elif op == 'stats': result = p.stats(source)
    elif op == 'head': result = p.view(source, limit=10)
    elif op == 'export': result = p.export(source, output, threads=threads)
    elif op == 'paf2pairs': result = p.convert(source, output, mode=op, threads=threads)
    elif op == 'pairs2cool': result = p.convert(source, output, mode=op, bin_size=10000, threads=threads)
    elif op == 'query':
        result = p.export(source, output, format='tsv', regions=[('chr1', 0, 1000000)],
                          index=config['index'], auto_index=False)
    else: raise ValueError(op)
    if op != 'head': print(json.dumps(result.to_dict()))


def run(command, directory, cpus, expected_stdout=None):
    metrics = directory / 'time.txt'
    stdout = directory / 'stdout.txt'
    stderr = directory / 'stderr.txt'
    start = time.perf_counter()
    # PIPE-based communicate wakes on EOF; file handles + wait(timeout) use
    # polling and can add up to 50ms to short-process timings on CPython.
    process = subprocess.run(['/usr/bin/time', '-f', '%e %U %S %M', '-o', str(metrics), *command],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=ROOT,
                             preexec_fn=lambda: os.sched_setaffinity(0, cpus), timeout=120)
    elapsed = time.perf_counter()-start
    stdout.write_bytes(process.stdout)
    stderr.write_bytes(process.stderr)
    if process.returncode: raise RuntimeError(stderr.read_text())
    _, user, system, rss = metrics.read_text().split()
    if expected_stdout is not None: assert stdout.read_text().strip() == expected_stdout
    return dict(wall_s=elapsed, cpu_s=float(user)+float(system), peak_rss_mib=int(rss)/1024), stdout


def digest(path):
    h = hashlib.sha256()
    with (gzip.open(path, 'rb') if path.suffix == '.gz' else path.open('rb')) as stream:
        for block in iter(lambda: stream.read(1024*1024), b''): h.update(block)
    return h.hexdigest()


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--worker':
        worker(json.loads(Path(sys.argv[2]).read_text()))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--rows', type=int, default=100000)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--consumer', type=Path, required=True, help='Python with h5py for independent Cooler checks')
    args = parser.parse_args()
    assert 10000 <= args.rows <= 1000000 and args.repetitions > 0
    import pqsio as p
    binary = ROOT / 'target/dev-release/pqsio'
    worker_code = 'import json, sys\n' + inspect.getsource(worker) + '\nworker(json.loads(sys.argv[1]))'

    cpus = sorted(os.sched_getaffinity(0))[:4]
    output_root = ROOT / 'tests/output'
    output_root.mkdir(exist_ok=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = dict(version=p.__version__, date=time.strftime('%Y-%m-%d %H:%M:%S %z'),
                  platform=platform.platform(), cpu_affinity=cpus, rows=args.rows,
                  repetitions=args.repetitions, profile='dev-release', loadavg_start=os.getloadavg(),
                  polars_threads=os.environ.get('POLARS_MAX_THREADS'), rayon_threads=os.environ.get('RAYON_NUM_THREADS'),
                  baseline='Minimal fresh Python process importing only json/sys/pqsio and calling the same Rust core via C ABI; not the removed rich-click CLI',
                  cache='No cache eviction; repeated warm-cache measurements; launch and imports included', measurements=[])
    with tempfile.TemporaryDirectory(prefix='cli-bench-', dir=output_root) as tmp:
        root = Path(tmp)
        pairs = root / 'pairs.pqs'
        contigs = {f'chr{i+1}':50000000 for i in range(4)}
        with p.PairsWriter(pairs, contigs, chunk_size=10000) as w:
            for lo in range(0,args.rows,10000):
                w.write_batch([p.Pair(str(i), (i//25000)%4, (i%25000)*1000+1,
                                     ((i//25000)+(i%5==0))%4, (i*15485863)%50000000+1,
                                     '+','-',0 if i%10==0 else 30)
                               for i in range(lo,min(lo+10000,args.rows))])
        paf = root / 'reads.paf'
        with paf.open('w') as out:
            for i in range(args.rows//4):
                for j in range(2):
                    start=(i*1009+j*15485863)%49999000
                    out.write(f'r{i}\t2000\t{j*1000}\t{j*1000+500}\t+\tchr{j+1}\t50000000\t{start}\t{start+500}\t490\t500\t30\n')
        result['paf_alignments'] = (args.rows//4)*2
        reference = {}
        def measure(label, backend, op, threads=1, index='off', repetitions=None):
            samples=[]
            for repetition in range(repetitions or args.repetitions):
                case=root / f'{label}-{backend}-t{threads}-{repetition}'
                case.mkdir()
                source=paf if op=='paf2pairs' else pairs
                suffix='.pairs.gz' if op=='export' else '.cool' if op=='pairs2cool' else '.pqs' if op=='paf2pairs' else '.tsv'
                output=case / ('output'+suffix)
                config=dict(op=op,source=str(source),output=str(output),threads=threads,index=index)
                if backend=='rust':
                    command=[str(binary),op]
                    if op=='version': command=[str(binary),'--version']
                    else: command += [str(source),'--no-progress']
                    if op in ('export','paf2pairs','pairs2cool'): command += ['-o',str(output),'-t',str(threads)]
                    if op=='pairs2cool': command += ['--bin-size','10k']
                    if op in ('info','stats'): command += ['--json']
                    if op=='query': command += ['--region','chr1:0-1000000','--index',index,'-o',str(output)]
                else:
                    conf=case / 'config.json';conf.write_text(json.dumps(config))
                    command=[sys.executable,'-c',worker_code,json.dumps(config)]
                metrics,stdout=run(command,case,cpus)
                if op in ('export','query'):
                    fingerprint=digest(output)
                    key='query' if op=='query' else 'export'
                    if key in reference: assert reference[key]==fingerprint,(label,backend)
                    else: reference[key]=fingerprint
                    metrics['output_bytes']=output.stat().st_size
                elif op in ('info','stats'):
                    fingerprint=json.loads(stdout.read_text())
                    if op in reference: assert reference[op]==fingerprint
                    else: reference[op]=fingerprint
                elif op=='head':
                    fingerprint=stdout.read_bytes()
                    assert len(fingerprint.splitlines())==11
                    if op in reference: assert reference[op]==fingerprint
                    else: reference[op]=fingerprint
                elif op=='paf2pairs':
                    report=p.info(output).to_dict();assert report['q0_records']==args.rows//4
                    exported=case / 'check.pairs';p.export(output,exported)
                    fingerprint=digest(exported)
                    if op in reference: assert reference[op]==fingerprint
                    else: reference[op]=fingerprint
                elif op=='pairs2cool':
                    code="""import h5py,hashlib,json,sys
with h5py.File(sys.argv[1]) as f:
 h=hashlib.sha256()
 for name in ['chroms/length','bins/chrom','bins/start','bins/end','pixels/bin1_id','pixels/bin2_id','pixels/count','indexes/bin1_offset','indexes/chrom_offset']:
  h.update(f[name][:].tobytes())
 assert int(f['pixels/count'][:].sum())==int(sys.argv[2])
 print(h.hexdigest())
"""
                    fingerprint=subprocess.check_output([str(args.consumer),'-c',code,str(output),str(args.rows)],text=True).strip()
                    if op in reference: assert reference[op]==fingerprint
                    else: reference[op]=fingerprint
                if op=='query': metrics['query_stats']=json.loads(stdout.read_text())['query_stats']
                samples.append(metrics)
            summary=dict(label=label,backend=backend,threads=threads,samples=samples,
                         median_wall_s=statistics.median(s['wall_s'] for s in samples),
                         median_cpu_s=statistics.median(s['cpu_s'] for s in samples),
                         median_peak_rss_mib=statistics.median(s['peak_rss_mib'] for s in samples))
            result['measurements'].append(summary)
            args.report.write_text(json.dumps(result,indent=2))
            print(label,backend,f"{summary['median_wall_s']:.4f}s",f"{summary['median_peak_rss_mib']:.1f}MiB",flush=True)
        for op in ['version','info','head','stats']:
            for backend in ['rust','python']: measure(op,backend,op,repetitions=10 if op=='version' else None)
        for op in ['paf2pairs','pairs2cool','export']:
            for threads in [1,4]:
                for backend in ['rust','python']: measure(op,backend,op,threads)
        for backend in ['rust','python']: measure('query_scan',backend,'query')
        # Repeated cold-index measurements use independent hard-linked fixture
        # directories; the original source is never modified by index creation.
        original=pairs
        for repetition in range(args.repetitions):
            pairs=root / f'index-fixture-{repetition}';pairs.mkdir()
            for name in ['_metadata','_metadata_counts','_contigsizes']: os.link(original/name,pairs/name)
            for quality in ['q0','q1']:
                (pairs/quality).mkdir()
                for file in (original/quality).glob('*.parquet'): os.link(file,pairs/quality/file.name)
            measure(f'query_build_{repetition}','rust','query',index='auto',repetitions=1)
        p.build_index(original)
        pairs=original
        for backend in ['rust','python']: measure('query_indexed',backend,'query',index='require')
    result['loadavg_end']=os.getloadavg()
    result['verified']='Output records/headers, statistics, query results and independently read Cooler arrays match across backends/thread counts'
    args.report.write_text(json.dumps(result,indent=2))


if __name__=='__main__': main()
