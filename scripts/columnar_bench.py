#!/usr/bin/env python3
"""Isolated-process row/column benchmark. Run through the project Pixi env.

Fresh child for every phase/sample; four pinned CPUs and four Polars/Rayon
threads. Data is synthetic, warm-cache, no fsync. No third-party Python imports.
"""
import argparse
from array import array
import hashlib
import json
import os
from pathlib import Path
import resource
import platform
import statistics
import subprocess
import sys
import tempfile
import time


p = None

def record(kind, i):
    if kind == "pairs":
        return p.Pair(f"r{i}",i%64,i*11%9000000+1,(i*7)%64,i*17%9000000+1,"+","-",(0,1,30,60)[i%4])
    return p.Alignment(i//8+1,8000,i%8*1000,i%8*1000+950,"+" if i%2 else "-",i%64,i*11%9000000,i*11%9000000+950,(0,1,30,60)[i%4],0.5,"pass")


def prepare(kind, mode, n):
    import pqsio as p
    batches=[]
    for a in range(0,n,4096):
        end=min(a+4096,n)
        if mode == "row":
            batches.append([record(kind,i) for i in range(a,end)])
        elif kind == "pairs":
            so, data=p.pack_strings(f"r{i}" for i in range(a,end))
            batches.append(p.PairColumns(so,data,array("I",(i%64 for i in range(a,end))),
                array("Q",(i*11%9000000+1 for i in range(a,end))),array("I",((i*7)%64 for i in range(a,end))),
                array("Q",(i*17%9000000+1 for i in range(a,end))),array("B",[43])*(end-a),
                array("B",[45])*(end-a),array("B",((0,1,30,60)[i%4] for i in range(a,end)))))
        else:
            so,data=p.pack_strings("pass" for _ in range(a,end))
            batches.append(p.ConcatColumns(array("Q",range(0,end-a+1,8)),array("Q",(i//8+1 for i in range(a,end))),
                array("I",[8000])*(end-a),array("I",(i%8*1000 for i in range(a,end))),
                array("I",(i%8*1000+950 for i in range(a,end))),array("B",(43 if i%2 else 45 for i in range(a,end))),
                array("I",(i%64 for i in range(a,end))),array("Q",(i*11%9000000 for i in range(a,end))),
                array("Q",(i*11%9000000+950 for i in range(a,end))),array("B",((0,1,30,60)[i%4] for i in range(a,end))),
                array("f",[0.5])*(end-a),so,data))
    return batches


def consume(reader, mode):
    """Consume every semantic field; no row-object expansion on column path."""
    import pqsio as p
    cls=p.PairColumns if reader.kind=="pairs" else p.ConcatColumns
    names=[n for n,t in cls._schema if not n.endswith(("_offsets","_bytes"))]
    sums={name:0 for name in names}; sums.update(string_bytes=0,string_lengths=0,records=0,reads=0)
    st="read_id" if reader.kind=="pairs" else "filter_reason"
    for b in reader.iter_batches() if mode=="row" else reader.iter_columns():
        sums["records"]+=len(b)
        for name in names:
            if mode=="column": value=sum(getattr(b,name))
            elif name.startswith("strand"): value=sum(ord(getattr(r,name)) for r in b)
            else: value=sum(getattr(r,name) for r in b)
            sums[name]+=value
        if mode=="column":
            data=getattr(b,st+"_bytes"); offsets=getattr(b,st+"_offsets")
            sums["string_bytes"]+=sum(data)
            sums["string_lengths"]+=sum(offsets[i+1]-offsets[i] for i in range(len(b)))
            if reader.kind=="concat": sums["reads"]+=len(b.read_offsets)-1
        else:
            for r in b:
                data=getattr(r,st).encode(); sums["string_bytes"]+=sum(data); sums["string_lengths"]+=len(data)
            if reader.kind=="concat": sums["reads"]+=sum(i==0 or b[i-1].read_idx!=r.read_idx for i,r in enumerate(b))
    return sums


def worker(args):
    global p
    import pqsio as p
    p._library() # dynamic loading excluded in both modes
    if args.phase=="read":
        start=time.perf_counter()
        with p.Reader(args.path) as r: checksum=consume(r,args.mode)
        elapsed=time.perf_counter()-start
        print(json.dumps(dict(seconds=elapsed,peak_rss_mib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss/1024,checksum=checksum)))
        return
    start=time.perf_counter()
    batches=prepare(args.kind,args.mode,args.rows)
    offsets={len(b):list(range(0,len(b)+1,8)) for b in batches} if args.kind=="concat" and args.mode=="row" else None
    prep=time.perf_counter()-start
    if args.phase=="prepared": start=time.perf_counter()
    writer=p.PairsWriter if args.kind=="pairs" else p.ConcatWriter
    with writer(args.path,[(f"ctg{i}",10000000) for i in range(64)],chunk_size=20000) as w:
        for b in batches:
            if args.mode=="column": w.write_columns(b)
            elif args.kind=="pairs": w.write_batch(b)
            else: w.write_batch(b,offsets[len(b)])
    elapsed=time.perf_counter()-start
    rss=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss/1024
    # Untimed, full-record verification against an independent row generator.
    expected=hashlib.sha256(); actual=hashlib.sha256()
    for i in range(args.rows): expected.update((repr(record(args.kind,i))+"\n").encode())
    with p.Reader(args.path) as r:
        for b in r.iter_batches():
            for row in b: actual.update((repr(row)+"\n").encode())
    assert actual.digest()==expected.digest()
    print(json.dumps(dict(seconds=elapsed,prepare_seconds=prep,peak_rss_mib=rss,digest=actual.hexdigest())))


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--rows",type=int,default=80000)
    ap.add_argument("--repetitions",type=int,default=5)
    ap.add_argument("--worker",action="store_true")
    ap.add_argument("--phase",choices=["prepared","e2e","read"])
    ap.add_argument("--kind",choices=["pairs","concat"])
    ap.add_argument("--mode",choices=["row","column"])
    ap.add_argument("--path")
    args=ap.parse_args()
    if args.worker: return worker(args)
    assert args.rows>0 and args.rows%8==0 and args.repetitions>=3
    root=Path(__file__).resolve().parents[1]
    env=dict(os.environ,POLARS_MAX_THREADS="4",RAYON_NUM_THREADS="4",PYTHONPATH=str(root/"python"),PQSIO_LIBRARY=str(root/"target/dev-release/libpqsio.so"))
    cpus=sorted(os.sched_getaffinity(0))[:4]
    os.sched_setaffinity(0,cpus)
    output=root/"tests/output"; output.mkdir(exist_ok=True)
    raw=[]; refs={}; readrefs={}
    with tempfile.TemporaryDirectory(prefix="columns-bench-",dir=output) as tmp:
        tmp=Path(tmp)
        def run(kind,mode,phase,path):
            cmd=[sys.executable,str(Path(__file__).resolve()),"--worker","--rows",str(args.rows),"--kind",kind,"--mode",mode,"--phase",phase,"--path",str(path)]
            result=subprocess.run(cmd,env=env,text=True,capture_output=True,check=True,timeout=120)
            return json.loads(result.stdout)
        # Shared read fixtures: build outside every read subprocess.
        for kind in ("pairs","concat"): run(kind,"row","prepared",tmp/(kind+"-read"))
        for rep in range(-1,args.repetitions): # one discarded warmup per case
            for kind in ("pairs","concat"):
                for phase in ("prepared","e2e","read"):
                    for mode in (("row","column") if rep%2==0 else ("column","row")):
                        path=tmp/(kind+"-read") if phase=="read" else tmp/f"{kind}-{phase}-{mode}-{rep}"
                        r=run(kind,mode,phase,path)
                        if phase=="read":
                            assert r["checksum"]==readrefs.setdefault(kind,r["checksum"])
                        else:
                            assert r["digest"]==refs.setdefault(kind,r["digest"])
                            import shutil
                            shutil.rmtree(path)
                        if rep>=0: raw.append(dict(kind=kind,phase=phase,mode=mode,rep=rep,**r))
            print(f"completed {'warmup' if rep<0 else 'repetition '+str(rep+1)}",flush=True)
    summary=[]
    for kind in ("pairs","concat"):
        for phase in ("prepared","e2e","read"):
            for mode in ("row","column"):
                group=[r for r in raw if (r["kind"],r["phase"],r["mode"])==(kind,phase,mode)]
                values=[r["seconds"] for r in group]; rss=[r["peak_rss_mib"] for r in group]
                summary.append(dict(kind=kind,phase=phase,mode=mode,median_seconds=statistics.median(values),min_seconds=min(values),max_seconds=max(values),median_rss_mib=statistics.median(rss),min_rss_mib=min(rss),max_rss_mib=max(rss),records_per_second=args.rows/statistics.median(values)))
    report=dict(python=platform.python_version(),platform=platform.platform(),rows=args.rows,repetitions=args.repetitions,warmups=1,cpus=cpus,threads=4,chunk_size=20000,batch_size=4096,compression="unchanged Polars default",summary=summary,samples=raw)
    (root/"docs/columnar-results.json").write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps(summary,indent=2))

if __name__=="__main__": main()
