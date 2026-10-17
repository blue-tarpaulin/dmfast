#!/usr/bin/env python3
"""Compatibility test of dmfast against one BYOND build, stock vs dmfast.

usage: compat.py [--phase PHASE] [--work DIR] [--suite-cache DIR] <BYOND version> <installer> <project.dme>

Checks: identical .dmb, verify mode, world load and output, bug fixes (fixtest.py), DM test suite
(dmtests.py). Writes a Markdown row to compat-<os>-<version>.md and $GITHUB_STEP_SUMMARY.
Phases (setup project verify load world fixes suite report) share state through --work; report fails the run.
"""
import argparse, json, os, re, shutil, statistics, subprocess, sys, tempfile, time, zipfile
import urllib.request
from itertools import takewhile
from pathlib import Path

import byond, dmtests, fixtest
from byond import DD, DM, OS, ROOT, WIN, world_lines

DD_ARGS = ["-trusted", "-invisible", "-ip", "127.0.0.1"]
S = {"fail": 0}  # results, kept in <work>/state.json between phases


def log(*a):
    print(*a, flush=True)


def fail(msg, extra=""):
    log(f"::error::{VERSION} {OS}: {msg}")
    if extra:
        log(extra.rstrip())
    S["fail"] = 1


def row(cells):
    line = "| " + " | ".join(map(str, cells)) + " |"
    Path(f"compat-{OS}-{VERSION}.md").write_text(line + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        open(os.environ["GITHUB_STEP_SUMMARY"], "a").write(line + "\n")


def stop(result, note):
    """A build with nothing to compare (no download, stock broken): record it, pass, no more phases."""
    log(f"== {VERSION} {OS}: {note}")
    row([VERSION, S.get("real", "-"), OS, result] + [""] * 11)
    S["done"] = 1
    save()
    sys.exit(0)


def save():
    (WORK / "state.json").write_text(json.dumps(S, indent=1))


# ---- running BYOND --------------------------------------------------------------------------------------------

def run(bin_dir, prog, args, cwd, limit=120, verify=False):
    """(exit code, output) of a BYOND program from that build, dmfast's stats on."""
    env = byond.env(bin_dir, DMFAST_STATS="1", **({"DMFAST_VERIFY": "1"} if verify else {}))
    return byond.run([bin_dir / prog, *args], cwd, env, limit)


def compile(bin_dir, dme, verify=False):
    """(rc, seconds, .dmb hash or None, output)."""
    dmb = dme.with_suffix(".dmb")
    dmb.unlink(missing_ok=True)
    t = time.monotonic()
    rc, out = run(bin_dir, DM, [dme.name], dme.parent, limit=300 if verify else 120, verify=verify)
    h = byond.sha16(dmb) if rc == 0 and dmb.exists() else None
    return rc, round(time.monotonic() - t, 1), h, out


def world(bin_dir, dmb, settle=None, limit=60):
    """(seconds to first line or None, still running, output); runs `settle` s more after it if given."""
    dmb.with_suffix(".dyn.rsc").unlink(missing_ok=True)  # a killed run can leave it half-written
    out = WORK / "world-raw.txt"
    env = byond.env(bin_dir, DMFAST_STATS="1")
    with open(out, "wb") as f:
        p = subprocess.Popen([str(bin_dir / DD), dmb.name, *DD_ARGS], cwd=dmb.parent, env=env, stdout=f, stderr=subprocess.STDOUT)
        start, first, end = time.monotonic(), None, None
        while p.poll() is None:
            now = time.monotonic()
            if first is None and world_lines(out.read_text("utf-8", "replace")):
                first = round(now - start, 2)
                if settle is None:
                    break
                end = now + settle
            elif (end or start + limit) <= now:
                break
            time.sleep(0.05)
        alive = p.poll() is None
        if alive:
            if WIN:
                subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)], capture_output=True)
            else:  # SIGKILL: a world may catch SIGTERM and take its time shutting down (tgstation does)
                p.kill()
            p.wait()
    return first, alive, out.read_text("utf-8", "replace")


def normalise(bin_dir, text):
    """World lines with run-specific paths, dates, addresses, times and durations masked."""
    out = []
    for l in world_lines(text):
        for p in {str(bin_dir), str(bin_dir).replace("\\", "/")}:
            l = l.replace(p, "#byond")
        l = re.sub(r"^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) [A-Z][a-z]{2} .*", "#date", l)
        l = re.sub(r"\d+(\.\d+){3}(:\d+)?", "#addr", l)
        l = re.sub(r"\d{1,2}:\d{2}:\d{2}", "#time", l)
        out.append(re.sub(r"\d+(\.\d+)? ?(ms|ds|s|secs?|seconds?|mins?|minutes?)\b", r"#\2", l))
    return out


def same_prefix(a, b):
    return sum(1 for _ in takewhile(lambda p: p[0] == p[1], zip(a, b)))


def dmfast_lines(out):
    return "\n".join(byond.dmfast_lines(out))


# ---- phases ---------------------------------------------------------------------------------------------------

def setup():
    url = f"https://byond-builds.dm-lang.org/{VERSION.split('.')[0]}/{VERSION}_byond{'' if WIN else '_linux'}.zip"
    zip_path = WORK / "byond.zip"
    for attempt in range(5):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "dmfast-compat"})  # the server refuses urllib's default
            with urllib.request.urlopen(req, timeout=30) as r, open(zip_path, "wb") as f:
                shutil.copyfileobj(r, f)
            break
        except urllib.error.HTTPError as e:
            if e.code == 404 and not WIN:  # client-only BYOND updates ship no Linux build
                stop("no Linux build", "skipped (no Linux build)")
            err = e
        except OSError as e:
            err = e
        time.sleep(5)
    else:
        log(f"::error::{VERSION} {OS}: download failed ({url}): {err}")
        sys.exit(1)
    with zipfile.ZipFile(zip_path) as z:
        z.extractall(WORK / "x")
    for f in (WORK / "x" / "byond" / "bin").iterdir():
        f.chmod(0o755)
    shutil.copytree(WORK / "x" / "byond" / "bin", STOCK)
    shutil.copytree(WORK / "x" / "byond" / "bin", FAST)
    p = subprocess.run(([] if WIN else ["sh"]) + [str(INSTALLER), str(FAST)], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if p.returncode != 0:
        log(p.stdout.decode("utf-8", "replace"))
        sys.exit(1)


def project():
    rc, secs, h, out = compile(STOCK, PROJECT)
    if rc == 124:
        stop(f"broken build: {PROJECT.name} compile timed out", "skipped (stock compile timed out)")
    if rc != 0:  # a project may refuse some BYOND builds itself (version checks)
        log(f"stock BYOND {VERSION} does not compile {PROJECT.name}; skipping its checks:")
        log("\n".join(l for l in out.splitlines() if "error" in l.lower())[:600])
        S.update(t_stock="n/a", t_fast="n/a")
        return
    S.update(t_stock=f"{secs}s", h_stock=h)
    rc, secs, h, out = compile(FAST, PROJECT)
    if rc != 0:
        fail(f"{PROJECT.name} compiles with stock but not with dmfast (exit {rc})", out)
    else:
        S.update(t_fast=f"{secs}s", h_fast=h)
        if h != S["h_stock"]:
            fail(f"{PROJECT.name} .dmb differs")
    S["compiler"] = dmfast_lines(out)


def verify():
    """Every patch with a verify mode works each answer out the original way too and counts mismatches."""
    outs = []
    if S.get("h_fast"):
        rc, _, h, out = compile(FAST, PROJECT, verify=True)
        outs.append(out)
        if rc != 0:
            fail(f"{PROJECT.name} verify-mode compile failed", out)
        elif h != S["h_stock"]:
            fail(f"{PROJECT.name} .dmb differs in verify mode")
    TW.mkdir(exist_ok=True)
    shutil.copy(ROOT / "tests" / "testworld.dme", TW)
    rc, _, _, out = compile(FAST, TW / "testworld.dme", verify=True)
    outs.append(out)
    if rc != 0:
        fail("testworld verify-mode compile failed", out)
    else:
        args = ["testworld.dmb", *DD_ARGS, "-logself"]
        rc, out = run(FAST, DD, args, TW, limit=300, verify=True)
        outs.append(out)
        if rc != 0:
            fail(f"testworld in verify mode exited {rc}", out)
        rc, out = run(FAST, DD, args, TW)  # the patch report comes from a plain run: verify mode turns groups off
        if rc != 0:
            fail(f"testworld exited {rc} with dmfast", out)
        S["runtime"] = dmfast_lines(out)
    text = "\n".join(outs)
    bad = [l for l in text.splitlines() if re.match(r"dmfast: .*mismatches=[1-9]", l)]
    if bad:
        fail("verify mode found mismatches", "\n".join(bad))
    S["verify"] = "FAIL" if bad else f"ok ({len(re.findall(r'^dmfast: .*mismatches=', text, re.M))} checks)"


def load():
    """Median seconds until the project's world prints its first line, 3 runs each."""
    if not S.get("h_fast"):
        return
    dmb = PROJECT.with_suffix(".dmb")
    if world(STOCK, dmb)[0] is None:
        log(f"stock DreamDaemon {VERSION} printed nothing from {PROJECT.name}'s world in 60 s; not timing world load")
        return
    for key, bin_dir in (("l_stock", STOCK), ("l_fast", FAST)):
        t = [x for x in (world(bin_dir, dmb)[0] for _ in range(3)) if x is not None]
        S[key] = f"{statistics.median(t)}s" if t else ""


def world_output():
    """dmfast prints the lines two stock runs agree on (10 s after load), and keeps running if stock does.
    tgstation rolls random station traits that can add a runtime: a mismatch must repeat on 3 dmfast runs."""
    S.setdefault("w_out", "n/a")
    if not S.get("l_stock"):
        return
    dmb, secs = PROJECT.with_suffix(".dmb"), 10
    (_, a1, t1), (_, a2, t2) = (world(STOCK, dmb, settle=secs) for _ in range(2))
    s1, s2 = normalise(STOCK, t1), normalise(STOCK, t2)
    n = same_prefix(s1, s2)
    for attempt in range(3):
        _, af, tf = world(FAST, dmb, settle=secs)
        f1 = normalise(FAST, tf)
        m = same_prefix(s1, f1)
        if m >= n and (af or not (a1 and a2)):
            break
        log(f"world output: dmfast run {attempt + 1} differs from stock at line {m + 1}")
    if n == 0:
        S["w_out"] = "n/a (stock not repeatable)"
    elif m < n:
        diff = "\n".join(f"< {a}\n> {b}" for a, b in zip(s1[m:n], f1[m:n] + [""] * n))[:2000]
        fail(f"{PROJECT.name}'s world prints differently with dmfast (line {m + 1} of the {n} that stock always prints):", diff)
        S["w_out"] = f"FAIL at line {m + 1}"
    elif a1 and a2 and not af:
        fail(f"{PROJECT.name}'s world exited within {secs} s of loading with dmfast, not with stock", "\n".join(f1[-20:]))
        S["w_out"] = "FAIL (exited)"
    else:
        S["w_out"] = f"same ({n} lines)"


def fixes():
    failed, S["fixes"] = fixtest.check(FAST, STOCK)
    if failed:
        fail("a bug fix's test failed")
        S["fixes"] = f"FAIL: {S['fixes']}"


def suite():
    S.setdefault("suite", "n/a")
    if (dmtests.SUITE / "Tests").is_dir():
        failed, S["suite"] = dmtests.check(FAST, STOCK, cache=SUITE_CACHE)
        if failed:
            fail("the DM test suite differs between stock and dmfast")
            S["suite"] = f"FAIL: {S['suite']}"


# The table columns a phase fills, for a phase CI cut at its time limit (compat-work/timeout-<phase>).
COLUMNS = {"project": ["t_fast"], "verify": ["verify"], "load": ["l_stock", "l_fast"], "world": ["w_out"],
           "fixes": ["fixes"], "suite": ["suite"]}


def report():
    for phase, keys in COLUMNS.items():
        if (WORK / f"timeout-{phase}").exists():
            log(f"::warning::{VERSION} {OS}: the {phase} step ran past its time limit")
            S.update({k: "timed out" for k in keys})
    mask = lambda t: (re.findall(r"patched=(0x[0-9a-f]+)", t or "") or ["?"])[-1]
    groups = (S.get("compiler") or "") + "\n" + (S.get("runtime") or "")
    disabled = ",".join(re.findall(r"^dmfast: disabled (\S+)", groups, re.M)) or "none"
    real = (re.findall(r"\d{3}\.\d{4}", run(STOCK, DM, [], WORK)[1]) or ["?"])[0]
    result = "FAIL" if S["fail"] else "pass"
    g = lambda k, d="n/a": S.get(k) or d
    log(f"== {VERSION} ({real}) {OS}: {result}")
    log(f"compile {PROJECT.name}: stock {g('t_stock')}, dmfast {g('t_fast')}, .dmb {g('h_stock')} / {g('h_fast')}")
    log(f"world load: stock {g('l_stock')}, dmfast {g('l_fast')}; output for 10 s after loading: {g('w_out')}")
    log(f"verify mode: {g('verify')}; bug fixes: {g('fixes', 'none')}; DM test suite: {g('suite')}")
    log(f"compiler groups: {mask(S.get('compiler'))}; runtime groups: {mask(S.get('runtime'))}; disabled: {disabled}")
    log(groups.strip())
    row([VERSION, real, OS, result, g("t_stock"), g("t_fast"), mask(S.get("compiler")), mask(S.get("runtime")), disabled,
         g("l_stock"), g("l_fast"), g("verify"), g("w_out"), g("fixes", "none"), g("suite")])
    return S["fail"]


PHASES = {"setup": setup, "project": project, "verify": verify, "load": load, "world": world_output,
          "fixes": fixes, "suite": suite, "report": report}

if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version"); ap.add_argument("installer"); ap.add_argument("project")
    ap.add_argument("--phase", default=os.environ.get("COMPAT_PHASE", "all"), choices=[*PHASES, "all"])
    ap.add_argument("--work", default=os.environ.get("COMPAT_WORK"))
    ap.add_argument("--suite-cache", default=os.environ.get("COMPAT_SUITE_CACHE"))
    a = ap.parse_args()
    VERSION, INSTALLER, PROJECT, SUITE_CACHE = a.version, Path(a.installer).resolve(), Path(a.project).resolve(), a.suite_cache
    if a.phase == "all":
        tmp = tempfile.TemporaryDirectory(prefix="compat-")
        WORK = Path(tmp.name)
    elif a.work:
        WORK = Path(a.work).resolve()
        WORK.mkdir(parents=True, exist_ok=True)
    else:
        ap.error("--phase needs --work (or COMPAT_WORK)")
    STOCK, FAST, TW = WORK / "stock", WORK / "fast" / "bin", WORK / "testworld"
    if (WORK / "state.json").exists():
        S.update(json.loads((WORK / "state.json").read_text()))
    if S.get("done"):
        sys.exit(0)
    rc = 0
    for name, fn in PHASES.items():
        if a.phase in ("all", name):
            rc = fn() or 0
            save()
    sys.exit(rc)
