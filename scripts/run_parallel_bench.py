"""Run small serial acceptance trials; generated data stays in ignored tests/output."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument("--rows", type=int, default=250000)
p.add_argument("--repetitions", type=int, default=6)
a = p.parse_args()
env = dict(os.environ, POLARS_MAX_THREADS="4", RAYON_NUM_THREADS="4")
cpus = sorted(os.sched_getaffinity(0))[:8]
exe = ROOT / "target/dev-release/examples/parallel_bench"
results = []
for kind in ("pairs", "concat"):
    for rep in range(-1, a.repetitions):
        modes = [0, 1, 2, 4, 8]
        if rep % 2:
            modes.reverse()
        for workers in modes:
            with tempfile.TemporaryDirectory(dir=ROOT/"tests/output", prefix="parallel-bench-") as tmp:
                cmd = ["taskset", "-c", ",".join(map(str, cpus)), str(exe), str(Path(tmp)/"data"), kind, str(workers), str(a.rows)]
                out = subprocess.run(cmd, env=env, check=True, capture_output=True, text=True, timeout=120).stdout.strip()
                values = dict(item.split("=", 1) for item in out.split())
                assert values["verified"] == "true"
                if rep >= 0:
                    results.append(dict(kind=kind, workers=workers, repetition=rep, seconds=float(values["seconds"]), rss_kib=int(values["rss_kib"])))
        print(kind, "warmup" if rep < 0 else "round %d" % (rep+1), "passed", flush=True)
report = {"rows": a.rows, "repetitions": a.repetitions, "cpus": cpus, "polars_threads":4, "results":results}
path = ROOT / "tests/output/parallel-results.json"
path.write_text(json.dumps(report, indent=2)+"\n")
for kind in ("pairs", "concat"):
    for workers in [0,1,2,4,8]:
        r = [v for v in results if v["kind"]==kind and v["workers"]==workers]
        print(kind, workers, "median_s=%.6f range=%.6f..%.6f median_mib=%.1f" %
              (statistics.median(v["seconds"] for v in r), min(v["seconds"] for v in r), max(v["seconds"] for v in r), statistics.median(v["rss_kib"] for v in r)/1024))
print(path)
