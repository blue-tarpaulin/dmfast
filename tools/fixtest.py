#!/usr/bin/env python3
"""Tests dmfast's opt-in bug fixes (BUGFIXES.md).

usage: fixtest.py [-s <stock bin dir>] <dmfast bin dir> [key...]

tests/fixes/<key>.dme must print <key>.expected with DMFAST_FIXES=<key> and =breaking, and not on stock.
tests/fixes/keep/*.dme must print the same on stock, safe and extended. A fix that didn't install is n/a.
Stock defaults to the _orig binaries beside dmfast. Exits non-zero on a failure.
"""
import argparse, difflib, os, re, shutil, subprocess, sys, tempfile
from pathlib import Path

import byond

TESTS = byond.ROOT / "tests" / "fixes"
# What BYOND prints once a world is up (the hub login's "connected", after any progress dots, and "Logged in
# as" can come mid-run), and its date line.
NOISE = re.compile(r"^(dmfast|Webclient users can visit|http://www\.byond\.com/play/|Or this code can be embedded:"
                   r"|<iframe src=\"http://www\.byond\.com/play/|The BYOND hub reports that port [0-9]+ is not reachable"
                   r"|\.*connected$|Logged in as )"
                   r"|^((Mon|Tue|Wed|Thu|Fri|Sat|Sun) [A-Z][a-z]{2} [ 0-9]{2} [0-9:]{8} [0-9]{4})?\s*$")


def world(dm, dd, fixes, tests, key, work):
    """(output as described above, dmfast's lines) of tests/<key>.dme built and run in a fresh dir."""
    t = work / "t"
    shutil.rmtree(t, ignore_errors=True)
    t.mkdir(parents=True)
    for f in tests.glob(f"{key}.*"):
        if f.suffix != ".expected":
            shutil.copy(f, t)
    for f in tests.glob(f"{key}-*"):
        (shutil.copytree if f.is_dir() else shutil.copy)(f, t / f.name)
    if (t / f"{key}-gen.sh").exists():
        subprocess.run([byond.bash(), f"./{key}-gen.sh"], cwd=t, check=True)
    env = byond.env(dm.parent, DMFAST_STATS="1", DMFAST_FIXES=fixes)
    _, dm_out = byond.run([dm, f"{key}.dme"], t, env, 120)
    out = ["dm: " + l for l in dm_out.splitlines() if not l.startswith("dmfast") and re.search(r"(error|warning):", l)]
    dd_out = log = ""
    if (t / f"{key}.dmb").exists():
        # -ip 127.0.0.1: loopback only, so Windows doesn't ask for a firewall rule for every fresh copy.
        rc, dd_out = byond.run([dd, f"{key}.dmb", "-trusted", "-invisible", "-logself", "-ip", "127.0.0.1"], t, env, 60)
        log_file = t / f"{key}.log"
        log = log_file.read_text("utf-8", "replace").replace("\r", "") if log_file.exists() else ""
        lines = log.splitlines()
        i = next((n for n, l in enumerate(lines) if "Welcome BYOND" in l), None)
        out += [l for l in ([] if i is None else lines[i + 1:]) if not NOISE.search(l)]
        if rc == byond.TIMEOUT:
            out.append("dd: timed out")
        elif byond.crashed(rc):
            out.append(f"dd: crashed (exit {rc})")
        elif rc:
            out.append(f"dd: exit {rc}")
    else:
        out.append("dm: no .dmb")
    # dmfast's own bug reports (an install-time reader used by a hook) fail the test.
    out += [l for l in byond.dmfast_lines(dm_out + dd_out) if l.startswith("dmfast: bug:")]
    text = "".join(l + "\n" for l in out)
    for d in (dm.parent, t):  # paths in messages differ between runs and machines
        for p in {str(d), d.as_posix()}:
            text = text.replace(p, "#dir")
    return text, byond.dmfast_lines(dm_out + dd_out + log)


def diff(a, b, la, lb):
    return "\n".join(list(difflib.unified_diff(a.splitlines(), b.splitlines(), la, lb, lineterm=""))[:24])


def check(fast, stock=None, keys=()):
    """Runs the tests (all, or keys; "keep" selects the regression worlds); prints each result and the
    "fixes: " line. Returns (failed, summary)."""
    fdm, fdd, sdm, sdd = byond.bins(fast, stock)
    keep = not keys or "keep" in keys
    keys = [k for k in keys if k != "keep"] or ([] if keys else sorted(f.stem for f in TESTS.glob("*.dme")))
    results, counts = [], {"pass": 0, "fail": 0, "na": 0}

    def result(name, kind, label):
        counts[kind] += 1
        results.append(f"{name} {label}")

    with tempfile.TemporaryDirectory(prefix="fixtest-") as tmp:
        work = Path(tmp)
        # Some builds (516.1660-1665) print nothing from any world: their empty results are n/a.
        probe = work / "probe"
        probe.mkdir()
        (probe / "probe.dme").write_text('/world/New()\n\tworld.log << "probe"\n\tshutdown()\n')
        silent = not world(sdm, sdd, "", probe, "probe", work)[0]

        for key in keys:
            if not (TESTS / f"{key}.dme").exists():
                print(f"::error::fix {key}: no tests/fixes/{key}.dme")
                result(key, "fail", "FAIL")
                continue
            fixed, stats = world(fdm, fdd, key, TESTS, key, work)
            if not any(l.startswith(f"dmfast: fix {key}: ") for l in stats):
                if "dm: no .dmb\n" in fixed:  # a DreamDaemon fix can't have installed if the world never ran
                    print(f"::error::fix {key}: the test didn't compile with DMFAST_FIXES={key}:\n{fixed}", end="")
                    result(key, "fail", "FAIL")
                    continue
                why = next((l.split(" - ", 1)[1] for l in stats if l.startswith(f"dmfast: disabled fix {key} - ")), "")
                print(f"fix {key}: n/a (didn't install{': ' + why if why else ''})")
                result(key, "na", "n/a")
                continue
            stock_out = world(sdm, sdd, "", TESTS, key, work)[0]
            all_out = world(fdm, fdd, "breaking", TESTS, key, work)[0]
            exp_file = TESTS / f"{key}.expected"
            if not exp_file.exists():
                print(f"::error::fix {key}: no tests/fixes/{key}.expected. Stock gave:\n{stock_out}With the fix:\n{fixed}", end="")
                result(key, "fail", "FAIL")
                continue
            expected = exp_file.read_text("utf-8").replace("\r", "")
            if silent and not fixed:
                print(f"fix {key}: n/a (worlds print nothing on this BYOND build)")
                result(key, "na", "n/a")
                continue
            bad = False
            for fx, got in ((key, fixed), ("breaking", all_out)):
                if got != expected:
                    print(f"::error::fix {key}: wrong output with DMFAST_FIXES={fx}:\n{diff(expected, got, 'expected', 'got')}")
                    bad = True
            if bad:
                result(key, "fail", "FAIL")
            elif stock_out == expected and key == "inodes" and os.stat(work).st_ino < 2**32:
                print("fix inodes: n/a (this filesystem has 32-bit inode numbers, so stock works too; the output with the fix is right)")
                result(key, "na", "n/a")
            elif stock_out == expected:
                print(f"::warning::fix {key}: stock BYOND already gives the expected output (fixed upstream, or the test doesn't reproduce the bug)")
                result(key, "na", "not needed")
            else:
                crash = " (stock BYOND crashes on this test, as expected)" if "\ndd: crashed" in "\n" + stock_out else ""
                print(f"fix {key}: pass{crash}")
                result(key, "pass", "ok")

        # Regression worlds: stock, safe and extended must agree.
        for f in sorted((TESTS / "keep").glob("*.dme")) if keep else []:
            name, keep_dir = f.stem, TESTS / "keep"
            stock_out = world(sdm, sdd, "", keep_dir, name, work)[0]
            bad = False
            for tier in ("safe", "extended"):
                got = world(fdm, fdd, tier, keep_dir, name, work)[0]
                if got != stock_out:
                    print(f"::error::keep {name}: output differs from stock with DMFAST_FIXES={tier}:\n{diff(stock_out, got, 'stock', tier)}")
                    bad = True
            if bad:
                result(f"keep/{name}", "fail", "FAIL")
            elif not stock_out:
                if silent:
                    result(f"keep/{name}", "na", "n/a")
                else:
                    print(f"::error::keep {name}: the world printed nothing")
                    result(f"keep/{name}", "fail", "FAIL")
            else:
                print(f"keep {name}: same as stock")
                result(f"keep/{name}", "pass", "ok")

    summary = f"{counts['pass']} pass" + (f", {counts['fail']} FAIL" if counts["fail"] else "") \
        + (f", {counts['na']} n/a" if counts["na"] else "") + f" ({', '.join(results)})"
    print(f"fixes: {summary}", flush=True)
    return counts["fail"] > 0, summary


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-s", dest="stock", help="stock BYOND bin dir")
    ap.add_argument("fast", help="bin dir with dmfast installed")
    ap.add_argument("keys", nargs="*")
    a = ap.parse_args()
    sys.exit(1 if check(a.fast, a.stock, a.keys)[0] else 0)
