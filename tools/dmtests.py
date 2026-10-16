#!/usr/bin/env python3
"""Runs the DM test suite (tests/dm-test-suite) on stock and dmfast and compares verdicts per test.

usage: dmtests.py [-s <stock bin dir>] [-f <fixes>] [-j <jobs>] [-c <cache dir>] <dmfast bin dir> [filter]

Verdicts: pass, compile, runtime, crash, expected-*, skip. Fails on a verdict difference, or a .dmb
difference with -f none (default). -c caches stock's verdicts per test. Stock defaults to the _orig binaries.
"""
import argparse, re, shutil, sys, tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import byond

SUITE = byond.ROOT / "tests" / "dm-test-suite" / "Tests"
TAG = re.compile(r"^// ?(COMPILE ERROR|RUNTIME ERROR|IGNORE|NOBYOND)")


class Side:
    """One side's verdict on a test, and what it printed (for the report)."""

    def __init__(self, verdict, dmb="-", compiler="", world=""):
        self.verdict, self.dmb, self.compiler, self.world = verdict, dmb, compiler, world

    def __str__(self):
        return f"{self.verdict} {self.dmb}"


def side(dm, dd, fixes, test, tag, t):
    """Compiles and runs one test in t (as the suite's own runner does) and judges it."""
    shutil.rmtree(t, ignore_errors=True)
    shutil.copytree(SUITE, t)
    rel = test.relative_to(SUITE).as_posix()
    # The suite's dm.sh: the test and the crash wrapper are #included on the .dme's third line.
    env_lines = (t / "environment.dme").read_bytes().splitlines(keepends=True)
    (t / "t.dme").write_bytes(b"".join(env_lines[:2]) + f'#include "{rel}"\n#include "crashwrapper.dm"\n'.encode()
                              + b"".join(env_lines[2:]))
    env = byond.env(dm.parent, DMFAST_FIXES=fixes)
    _, out = byond.run([dm, "-max_errors", "0", "t.dme"], t, env, 120)
    compiler = "\n".join(l for l in out.splitlines() if re.search(r"(error|warning):", l))
    if "- 0 errors, " not in out or not (t / "t.dmb").exists():
        return Side("pass" if tag == "COMPILE ERROR" else "compile", compiler=compiler)
    h = byond.sha16(t / "t.dmb")
    if tag == "COMPILE ERROR":
        return Side("expected-compile", h, compiler)
    rc, _ = byond.run([dd, "t.dmb", "-once", "-close", "-trusted", "-invisible", "-log", "errors.log", "-ip", "127.0.0.1"], t, env, 60)
    if rc:
        return Side("crash", h, compiler)
    log = t / "errors.log"
    printed = byond.world_lines(log.read_text("utf-8", "replace")) if log.exists() else []
    if printed:
        v = "pass" if tag == "RUNTIME ERROR" else "runtime"
    else:
        v = "expected-runtime" if tag == "RUNTIME ERROR" else "pass"
    return Side(v, h, compiler, "\n".join(printed))


def one(n, test, a, work):
    """(test path, stock, dmfast). DEBUG stamps the .dmb: a stock hash that changes on recompile becomes "~"."""
    rel = test.relative_to(SUITE).as_posix()
    head = test.read_text("utf-8", "replace").splitlines()[:2]
    tag = next((m.group(1) for m in map(TAG.match, head) if m), "")
    if tag in ("IGNORE", "NOBYOND"):
        return rel, Side("skip"), Side("skip")
    t = work / str(n)
    try:
        cached = a.cache / (rel.replace("/", "_") + ".verdict") if a.cache else None
        if cached and cached.exists() and cached.stat().st_size:
            s = Side(*cached.read_text().split())
        else:
            s = side(a.sdm, a.sdd, "none", test, tag, t / "s")
            if cached:
                cached.write_text(f"{s}\n")
        f = side(a.fdm, a.fdd, a.fixes, test, tag, t / "f")
        if s.dmb != f.dmb and "-" not in (s.dmb, f.dmb):
            if str(side(a.sdm, a.sdd, "none", test, tag, t / "s2")) != str(s):
                s.dmb = f.dmb = "~"
        return rel, s, f
    except Exception as e:  # anything failing here counts as a crash of that test, not of the run
        return rel, Side("crash"), Side("crash", world=repr(e))
    finally:
        shutil.rmtree(t, ignore_errors=True)


def check(fast, stock=None, fixes="none", jobs=4, cache=None, filter=""):
    """Runs the suite; prints the report and the "dmtests: " line. Returns (failed, summary)."""
    if not (SUITE / "Tests").is_dir():
        print("tests/dm-test-suite is empty: git submodule update --init")
        return True, "no suite"
    a = argparse.Namespace(fixes=fixes, cache=Path(cache).resolve() if cache else None)
    a.fdm, a.fdd, a.sdm, a.sdd = byond.bins(fast, stock)
    if a.cache:
        a.cache.mkdir(parents=True, exist_ok=True)
    tests = sorted(p for p in (SUITE / "Tests").rglob("*.dm") if filter in p.as_posix())
    if not tests:
        print(f"dmtests: no tests match '{filter}'")
        return True, "no tests"
    with tempfile.TemporaryDirectory(prefix="dmtests-") as tmp, ThreadPoolExecutor(jobs) as pool:
        rows = sorted(pool.map(lambda nt: one(nt[0], nt[1], a, Path(tmp)), enumerate(tests)), key=lambda r: r[0])

    total = sum(1 for _, s, _ in rows if s.verdict != "skip")
    diffs = [r for r in rows if r[1].verdict != r[2].verdict and r[1].verdict != "crash"]
    masked = [f"{rel} (dmfast: {f.verdict})" for rel, s, f in rows if s.verdict == "crash" and f.verdict != "crash"]
    dmbdiff = [rel for rel, s, f in rows if "-" not in (s.dmb, f.dmb) and s.dmb != f.dmb]
    fails = lambda i: [f"{r[0]} ({r[i].verdict})" for r in rows if r[i].verdict not in ("pass", "skip")]
    sfail, ffail = fails(1), fails(2)
    indent = lambda xs: "\n".join("  " + x for x in xs)
    if diffs:
        print("::error::dmtests: verdict differs between stock and dmfast (test, stock, dmfast):")
        print(indent(f"{rel}\t{s}\t{f}" for rel, s, f in diffs))
        for rel, s, f in diffs:
            print(f"== {rel}")
            for label, x in (("stock", s), ("dmfast", f)):
                print("\n".join(f"--- {label} compiler:\n{x.compiler}\n--- world:\n{x.world}".splitlines()[:40]))
    if masked:
        print("::notice::dmtests: stock BYOND crashes, dmfast does not (its allocator doesn't trip on BYOND's memory bug):")
        print(indent(masked))
    if dmbdiff:
        print(f"::{'error' if fixes == 'none' else 'notice'}::dmtests: .dmb differs from stock (DMFAST_FIXES={fixes}):")
        print(indent(dmbdiff))
    if sfail:
        print("stock BYOND fails:\n" + indent(sfail))
    if ffail:
        print(f"dmfast (DMFAST_FIXES={fixes}) fails:\n" + indent(ffail))
    summary = (f"{total} tests, {len(diffs)} verdict differences, {len(dmbdiff)} .dmb differences; "
               f"stock fails {len(sfail)}, dmfast fails {len(ffail)}")
    print(f"dmtests: {summary}", flush=True)
    return bool(diffs) or (fixes == "none" and bool(dmbdiff)), summary


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-s", dest="stock", help="stock BYOND bin dir")
    ap.add_argument("-f", dest="fixes", default="none", help="DMFAST_FIXES for the dmfast side")
    ap.add_argument("-j", dest="jobs", type=int, default=4)
    ap.add_argument("-c", dest="cache", help="cache dir for stock's verdicts")
    ap.add_argument("fast", help="bin dir with dmfast installed")
    ap.add_argument("filter", nargs="?", default="")
    a = ap.parse_args()
    sys.exit(1 if check(a.fast, a.stock, a.fixes, a.jobs, a.cache, a.filter)[0] else 0)
