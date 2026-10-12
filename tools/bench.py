"""Paired A/B timing of two dm.exe builds (or one build with two environments).

usage: py tools/bench.py --dme X:\\repos\\project\\project.dme --a <bin dir> --b <bin dir>
       [--a-env K=V ...] [--b-env K=V ...] [--pairs 10] [--clean-rsc] [--json out.json]

Runs a warm-up build, then N interleaved pairs (alternating which side goes first) so slow periods on the
machine hit both sides equally. Per run: wall time, user/kernel CPU time and CPU cycles of the dm.exe
process (QueryProcessCycleTime, the least noisy). Reports per-side medians and the mean paired difference
(B - A) with a 95% confidence interval; a difference whose interval excludes 0 is real at that level.
Every run must exit 0, and every .dmb must match the first run's.

--clean-rsc deletes the project's .rsc files before each build (measures .rsc rebuilds) and restores
the originals afterwards.
"""
import argparse
import ctypes
import glob
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes

k32 = ctypes.WinDLL('kernel32', use_last_error=True)
HIGH_PRIORITY_CLASS = 0x80
# Two-sided 95% t quantiles by degrees of freedom.
T95 = {1: 12.71, 2: 4.30, 3: 3.18, 4: 2.78, 5: 2.57, 6: 2.45, 7: 2.36, 8: 2.31, 9: 2.26, 10: 2.23, 11: 2.20,
       12: 2.18, 13: 2.16, 14: 2.14, 15: 2.13, 19: 2.09, 24: 2.06, 29: 2.05}


def t95(df):
    return T95.get(df) or T95[max(k for k in T95 if k <= df)]


def env_pairs(items):
    return dict(i.split('=', 1) for i in items or [])


def sha(path):
    with open(path, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


def run(bin_dir, dme, env):
    ft = lambda f: (f.dwHighDateTime << 32 | f.dwLowDateTime) / 1e7
    t0 = time.perf_counter()
    p = subprocess.Popen([os.path.join(bin_dir, 'dm.exe'), dme], cwd=os.path.dirname(dme), stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, env={**os.environ, **env}, creationflags=HIGH_PRIORITY_CLASS)
    p.wait()
    wall = time.perf_counter() - t0
    h = wintypes.HANDLE(int(p._handle))
    c, e, kt, ut = (wintypes.FILETIME() for _ in range(4))
    k32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e), ctypes.byref(kt), ctypes.byref(ut))
    cycles = ctypes.c_uint64()
    k32.QueryProcessCycleTime(h, ctypes.byref(cycles))
    if p.returncode != 0:
        sys.exit(f'{bin_dir}: dm.exe exited {p.returncode}')
    return dict(wall=wall, user=ft(ut), kernel=ft(kt), gcycles=cycles.value / 1e9)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--dme', required=True)
    ap.add_argument('--a', required=True)
    ap.add_argument('--b', required=True)
    ap.add_argument('--a-env', nargs='*')
    ap.add_argument('--b-env', nargs='*')
    ap.add_argument('--pairs', type=int, default=10)
    ap.add_argument('--clean-rsc', action='store_true')
    ap.add_argument('--json')
    a = ap.parse_args()
    dme = os.path.abspath(a.dme)
    dmb = dme[:-4] + '.dmb'
    rsc_glob = dme[:-4] + '*.rsc'
    sides = {'A': (a.a, env_pairs(a.a_env)), 'B': (a.b, env_pairs(a.b_env))}

    backup = tempfile.mkdtemp(prefix='bench-rsc-') if a.clean_rsc else None
    if backup:
        for f in glob.glob(rsc_glob):
            shutil.copy2(f, backup)

    def build(side):
        if backup:
            for f in glob.glob(rsc_glob):
                os.remove(f)
        bin_dir, env = sides[side]
        return run(bin_dir, dme, env)

    try:
        build('A')  # warm-up: file cache, .rsc state
        want = sha(dmb)
        res = {'A': [], 'B': []}
        for i in range(a.pairs):
            for side in (('A', 'B') if i % 2 == 0 else ('B', 'A')):
                res[side].append(build(side))
                if sha(dmb) != want:
                    sys.exit(f'run {i} side {side}: .dmb differs from the warm-up build')
            print(f'pair {i + 1}/{a.pairs}: A {res["A"][-1]["wall"]:.2f}s  B {res["B"][-1]["wall"]:.2f}s', flush=True)
    finally:
        if backup:
            for f in glob.glob(rsc_glob):
                os.remove(f)
            for f in glob.glob(os.path.join(backup, '*')):
                shutil.copy2(f, os.path.dirname(dme))
            shutil.rmtree(backup)

    n = a.pairs
    print(f'\n{"metric":<9}{"A min":>9}{"B min":>9}{"B-A min":>9}{"A med":>9}{"B med":>9}{"B-A mean":>10}'
          f'{"95% CI":>22}{"trim20":>9}{"B-A %":>8}')
    out = {}
    for m in ('wall', 'user', 'kernel', 'gcycles'):
        va, vb = [r[m] for r in res['A']], [r[m] for r in res['B']]
        d = [y - x for x, y in zip(va, vb)]
        mean = statistics.fmean(d)
        half = t95(n - 1) * statistics.stdev(d) / n ** 0.5 if n > 1 else float('nan')
        cut = n // 5
        trim = statistics.fmean(sorted(d)[cut:n - cut]) if n - 2 * cut > 0 else mean
        pct = (min(vb) - min(va)) / min(va) * 100
        real = '  *' if abs(mean) > half else ''
        print(f'{m:<9}{min(va):>9.3f}{min(vb):>9.3f}{min(vb) - min(va):>+9.3f}{statistics.median(va):>9.3f}'
              f'{statistics.median(vb):>9.3f}{mean:>+10.3f}   [{mean - half:+.3f}, {mean + half:+.3f}]{trim:>+9.3f}'
              f'{pct:>+7.1f}%{real}')
        out[m] = dict(a=va, b=vb, mean_diff=mean, ci95=half, trimmed_diff=trim)
    print('B-A % is of the minimums; * = mean difference\'s 95% interval excludes 0')
    if a.json:
        with open(a.json, 'w') as f:
            json.dump(out, f, indent=1)


if __name__ == '__main__':
    main()
