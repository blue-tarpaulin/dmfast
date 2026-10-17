#!/usr/bin/env python3
"""Runs the DM test suite (tests/dm-test-suite) on stock and dmfast and compares verdicts per test.

usage: dmtests.py [-s <stock bin dir>] [-f <fixes>] [-j <jobs>] [-c <cache dir>] <dmfast bin dir> [filter]

Verdicts: pass, compile, runtime, crash, expected-*, skip. Fails on a verdict difference, or a .dmb
difference with -f none (default). -c caches stock's verdicts per test. Stock defaults to the _orig binaries.
"""
import argparse, os, queue, re, shutil, sys, tempfile, threading
from concurrent.futures import ThreadPoolExecutor
from contextlib import nullcontext
from pathlib import Path

import byond

SUITE = byond.ROOT / "tests" / "dm-test-suite" / "Tests"
TAG = re.compile(r"^// ?(COMPILE ERROR|RUNTIME ERROR|IGNORE|NOBYOND)")
# A world takes about a second to start and stop, nearly all of it idle: a few tests per core at once.
JOBS = min(16, 4 * (os.cpu_count() or 1))
FILES, DIRS = {}, set()  # the suite's files (relative path -> size, mtime) and directories
# Worlds started in the same second give their temporary savefiles the same names, in the cache of BYOND's user
# directory, and one of them fails ("file is in use"). Savefile tests run one at a time.
SAVEFILES = threading.Lock()


class Side:
    """One side's verdict on a test, and what it printed (for the report)."""

    def __init__(self, verdict, dmb="-", compiler="", world=""):
        self.verdict, self.dmb, self.compiler, self.world = verdict, dmb, compiler, world

    def __str__(self):
        return f"{self.verdict} {self.dmb}"


def snapshot():
    FILES.clear()
    DIRS.clear()
    for dp, dirs, files in os.walk(SUITE):
        rel = Path(dp).relative_to(SUITE)
        DIRS.update((rel / x).as_posix() for x in dirs)
        for x in files:
            st = (Path(dp) / x).stat()
            FILES[(rel / x).as_posix()] = st.st_size, st.st_mtime_ns


def fresh(t):
    """Makes t a copy of the suite again: drops what the last run added or changed, copies back what it removed. A
    new copy for every run took as long as the run on Windows. Copies keep size and mtime."""
    if not t.exists():
        shutil.copytree(SUITE, t)
        return
    kept = set()
    for dp, dirs, files in os.walk(t):
        d, rel = Path(dp), Path(dp).relative_to(t)
        for x in [x for x in dirs if (rel / x).as_posix() not in DIRS]:
            shutil.rmtree(d / x)
            dirs.remove(x)
        for x in files:
            st = (d / x).stat()
            if FILES.get((rel / x).as_posix()) == (st.st_size, st.st_mtime_ns):
                kept.add((rel / x).as_posix())
            else:
                (d / x).unlink()
    for x in FILES.keys() - kept:
        (t / x).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SUITE / x, t / x)


def build(dm, fixes, test, t):
    """Compiles the test in t, a fresh copy of the suite, as the suite's dm.sh does: (env, output, .dmb hash or "-")."""
    fresh(t)
    rel = test.relative_to(SUITE).as_posix()
    # The suite's dm.sh: the test and the crash wrapper are #included on the .dme's third line.
    env_lines = (t / "environment.dme").read_bytes().splitlines(keepends=True)
    (t / "t.dme").write_bytes(b"".join(env_lines[:2]) + f'#include "{rel}"\n#include "crashwrapper.dm"\n'.encode()
                              + b"".join(env_lines[2:]))
    env = byond.env(dm.parent, DMFAST_FIXES=fixes)
    _, out = byond.run([dm, "-max_errors", "0", "t.dme"], t, env, 120)
    ok = "- 0 errors, " in out and (t / "t.dmb").exists()
    return env, out, byond.sha16(t / "t.dmb") if ok else "-"


def side(dm, dd, fixes, test, tag, t):
    """Compiles and runs one test in t (as the suite's own runner does) and judges it."""
    env, out, h = build(dm, fixes, test, t)
    compiler = "\n".join(l for l in out.splitlines() if re.search(r"(error|warning):", l))
    if h == "-":
        return Side("pass" if tag == "COMPILE ERROR" else "compile", compiler=compiler)
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


def one(test, a):
    """(test path, stock, dmfast). Both sides run in the same directory, a worker's: the .dmb holds its path."""
    rel = test.relative_to(SUITE).as_posix()
    text = test.read_text("utf-8", "replace")
    tag = next((m.group(1) for m in map(TAG.match, text.splitlines()[:2]) if m), "")
    if tag in ("IGNORE", "NOBYOND"):
        return rel, Side("skip"), Side("skip")
    t = a.slots.get()
    try:
        with SAVEFILES if "savefile" in text else nullcontext():
            cached = a.cache / (rel.replace("/", "_") + ".verdict") if a.cache else None
            if cached and cached.exists() and cached.stat().st_size:
                s = Side(*cached.read_text().split())
            else:
                s = side(a.sdm, a.sdd, "none", test, tag, t)
                if cached:
                    cached.write_text(f"{s}\n")
            f = side(a.fdm, a.fdd, a.fixes, test, tag, t)
        if s.dmb != f.dmb and "-" not in (s.dmb, f.dmb):
            # A cached stock hash is from another directory: build stock here again (no run). A stock build that
            # differs from its own rebuild here makes both "~".
            h = build(a.sdm, "none", test, t)[2]
            if h != f.dmb and h != build(a.sdm, "none", test, t)[2]:
                h = f.dmb = "~"
            s.dmb = h
        return rel, s, f
    except Exception as e:  # anything failing here counts as a crash of that test, not of the run
        t = t.with_name(t.name + "+")  # and the next test gets a new copy of the suite
        return rel, Side("crash"), Side("crash", world=repr(e))
    finally:
        a.slots.put(t)


def check(fast, stock=None, fixes="none", jobs=None, cache=None, filter=""):
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
    jobs = jobs or JOBS
    snapshot()
    with tempfile.TemporaryDirectory(prefix="dmtests-") as tmp, ThreadPoolExecutor(jobs) as pool:
        a.slots = queue.SimpleQueue()  # a copy of the suite per worker, reused
        for n in range(jobs):
            a.slots.put(Path(tmp) / str(n))
        rows = sorted(pool.map(lambda test: one(test, a), tests), key=lambda r: r[0])

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
    ap.add_argument("-j", dest="jobs", type=int, default=JOBS, help=f"tests at once (default {JOBS})")
    ap.add_argument("-c", dest="cache", help="cache dir for stock's verdicts")
    ap.add_argument("fast", help="bin dir with dmfast installed")
    ap.add_argument("filter", nargs="?", default="")
    a = ap.parse_args()
    sys.exit(1 if check(a.fast, a.stock, a.fixes, a.jobs, a.cache, a.filter)[0] else 0)
