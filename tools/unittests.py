#!/usr/bin/env python3
"""Runs an SS13 codebase's own CI unit tests (or a long soak) on DreamDaemon, stock vs dmfast, and compares.

  unittests.py prep <work> --src <codebase> --dme <x.dme> --bin <dmfast bin> [-D DEFINE ...] [--ci MAP] [--lib F ...]
                   [--setup BASH]
  unittests.py run <work> --bin <dmfast bin> --ip <host> --port <n> [--mode M ...] [--soak SECS] [--topic Q ...]
  unittests.py compare <work> [label ...]

prep copies the codebase to <work>/src (never touches the original), adds --lib files (rust_g.dll, ...),
compiles it with dmfast's DreamMaker (its .dmb is byte-identical to stock's; the sha is printed) and picks
the run dir: with --ci MAP it deploys like tgstation's tools/ci/run_server.sh (tools/deploy.sh ci_test,
tools/ci/ci_config.txt without SQL_ENABLED, next_map.json from _maps/map_jsons/MAP.json, or _maps/MAP.json in
older forks; tgstation and its forks); without, the world runs in <work>/src. --setup runs in the run dir after
that (config edits, a map file). data/ and config/ are then saved so every run starts the same.

run runs the world once per mode, one after another, each from the saved state:
  stock     the _orig DreamDaemon beside dmfast
  default   dmfast, no fixes
  safe | extended | breaking   dmfast with DMFAST_FIXES=<tier>
  LABEL:VAR=VAL[:VAR=VAL]       dmfast with that environment (e.g. nojit:DMFAST_DISABLE=foo)
CI runs end when the world does (-close; --timeout kills it). --soak SECS boots without -close, sends
--topic queries (world/Topic, e.g. ?ping) every --every seconds and kills the world after SECS.
Memory (working set, private bytes) is sampled every 5 s. Per mode: <work>/out/<label>/{console.log,
mem.csv, files/ (everything the run wrote), summary.json}.

compare prints, for the given labels (default: all runs), exit/crash, clean_run.lk, test counts, every test
whose result differs, runtime totals and each runtime message whose count differs, memory and BUG lines.
Exits 1 when results differ.
"""
import argparse, collections, json, os, re, shutil, socket, struct, subprocess, sys, time
from pathlib import Path

import byond

try:
    import psutil
except ImportError:
    psutil = None

TIERS = ("safe", "extended", "breaking")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
CRASH = r"^BUG:|byondcore\.dll \+ 0x|libbyond\.so|Segmentation fault|Aborted"
STAT = {0: "pass", 1: "FAIL", 2: "skip"}


def sh(cmd, cwd):
    subprocess.run([byond.bash(), "-c", cmd], cwd=cwd, check=True)


def manifest(d):
    return sorted(str(p.relative_to(d)).replace("\\", "/") for p in d.rglob("*") if p.is_file())


# --- prep -----------------------------------------------------------------------------------------------

def prep(a):
    work, src = Path(a.work).resolve(), Path(a.src).resolve()
    s = work / "src"
    if s.exists():
        shutil.rmtree(s)
    work.mkdir(parents=True, exist_ok=True)
    stem = Path(a.dme).stem
    ignore = shutil.ignore_patterns(".git", "node_modules", f"{stem}.dmb", f"{stem}.rsc", f"{stem}.dyn.rsc",
                                    f"{stem}.int", "*.lk", "ci_test")
    shutil.copytree(src, s, ignore=ignore)
    for f in a.lib:
        shutil.copy(f, s)
    dm = Path(a.bin).resolve() / byond.DM
    t = time.time()
    rc, out = byond.run([dm] + [f"-D{d}" for d in a.D] + [a.dme], s, byond.env(dm.parent), 3600)
    dmb = s / f"{stem}.dmb"
    if rc or not dmb.exists():
        sys.exit(f"compile failed ({rc}):\n{out[-3000:]}")
    print(f"compiled in {time.time() - t:.0f}s: {dmb.name} sha {byond.sha16(dmb)} "
          f"{[l for l in out.splitlines() if 'error' in l][-1:]}")
    if a.ci:
        run_dir = s / "ci_test"
        sh("bash tools/deploy.sh ci_test", s)
        (run_dir / "config").mkdir(exist_ok=True)
        (run_dir / "data").mkdir(exist_ok=True)
        cfg = (s / "tools/ci/ci_config.txt").read_text()
        (run_dir / "config/config.txt").write_text(
            "".join(l for l in cfg.splitlines(True) if not l.startswith("SQL_ENABLED")))
        jsons = s / "_maps/map_jsons" if (s / "_maps/map_jsons").is_dir() else s / "_maps"
        shutil.copy(jsons / f"{a.ci}.json", run_dir / "data/next_map.json")
        # map_config as tgstation's collect_data.yml builds it: every map json not exclude_from_ci
        maps = [j.stem for j in sorted(jsons.glob("*.json"))
                if not json.loads(j.read_text(encoding="utf-8")).get("exclude_from_ci")]
        (run_dir / "config/maps.txt").write_text("".join(f"map {m}\nendmap\n" for m in maps))
        for f in a.lib:  # deploy.sh copies DLLs only under Msys
            shutil.copy(f, run_dir)
        params = "log-directory=ci"
    else:
        run_dir = s
        params = ""
    if a.setup:
        sh(a.setup, run_dir)
    pristine = work / "pristine"
    shutil.rmtree(pristine, ignore_errors=True)
    for d in ("data", "config"):
        if (run_dir / d).is_dir():
            shutil.copytree(run_dir / d, pristine / d)
    (work / "state.json").write_text(json.dumps({
        "run_dir": str(run_dir), "dmb": dmb.name, "params": params, "ci": bool(a.ci),
        "manifest": manifest(run_dir), "dmb_sha": byond.sha16(dmb), "src": str(src), "defines": a.D}, indent=1))
    print(f"run dir {run_dir}")


def reset(st, work):
    """Back to the state prep left: files the last run created removed, data/ and config/ restored."""
    run_dir = Path(st["run_dir"])
    keep = set(st["manifest"])
    for d in ("data", "config"):
        shutil.rmtree(run_dir / d, ignore_errors=True)
        if (work / "pristine" / d).exists():
            shutil.copytree(work / "pristine" / d, run_dir / d)
    for p in manifest(run_dir):
        if p not in keep:
            (run_dir / p).unlink()


def collect(st, work, dest):
    """Copies what a run wrote (files not in the prep manifest, and data/ files it changed) to dest."""
    run_dir = Path(st["run_dir"])
    keep = set(st["manifest"])
    for p in manifest(run_dir):
        orig = work / "pristine" / p
        if p not in keep or (p.startswith("data/") and (not orig.exists() or
                                                        orig.read_bytes() != (run_dir / p).read_bytes())):
            if p.endswith(".rsc"):
                continue
            (dest / p).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(run_dir / p, dest / p)


# --- run ------------------------------------------------------------------------------------------------

def topic(host, port, query, timeout=5.0):
    """world/Topic over BYOND's TCP protocol: (seconds, reply) or (seconds, None) on failure."""
    q = query.encode() + b"\0"
    pkt = struct.pack(">HH", 0x83, len(q) + 5) + b"\0" * 5 + q  # length: 5 pad bytes + query + NUL
    t = time.time()
    try:
        with socket.create_connection((host, port), timeout) as c:
            c.sendall(pkt)
            head = b""
            while len(head) < 4:
                b = c.recv(4 - len(head))
                if not b:
                    return time.time() - t, None
                head += b
            n = struct.unpack(">H", head[2:4])[0]
            body = b""
            while len(body) < n:
                b = c.recv(n - len(body))
                if not b:
                    break
                body += b
    except OSError:
        return time.time() - t, None
    if body[:1] == b"\x2a" and len(body) >= 5:
        reply = str(struct.unpack("<f", body[1:5])[0])
    elif body[:1] == b"\x06":
        reply = body[1:].rstrip(b"\0").decode("utf-8", "replace")
    else:
        reply = "null" if not body or body[:1] == b"\0" else repr(body[:40])
    return time.time() - t, reply


def mode_env(spec, bin_dir):
    """(label, DreamDaemon, extra environment) of a --mode."""
    label, *kv = spec.split(":")
    fast_dd = bin_dir / byond.DD
    if label == "stock":
        return label, bin_dir / byond.DD_ORIG, {}
    if label in TIERS:
        return label, fast_dd, {"DMFAST_FIXES": label}
    return label, fast_dd, dict(x.split("=", 1) for x in kv)


def mem(pid):
    """(working set MB, private MB, threads); Linux: VmRSS, RssAnon, Threads from /proc without psutil."""
    try:
        if psutil:
            p = psutil.Process(pid)
            m = p.memory_info()
            return m.rss >> 20, getattr(m, "private", getattr(m, "vms", 0)) >> 20, p.num_threads()
        s = dict(l.split(":", 1) for l in Path(f"/proc/{pid}/status").read_text().splitlines() if ":" in l)
        kb = lambda k: int(s[k].split()[0]) >> 10
        return kb("VmRSS"), kb("RssAnon"), int(s["Threads"])
    except Exception:
        return None


def run(a):
    work = Path(a.work).resolve()
    st = json.loads((work / "state.json").read_text())
    run_dir, bin_dir = Path(st["run_dir"]), Path(a.bin).resolve()
    if a.ip == "127.0.0.1":
        sys.exit("use your own loopback host")
    for spec in a.mode:
        label, dd, extra = mode_env(spec, bin_dir)
        out = work / "out" / label
        shutil.rmtree(out, ignore_errors=True)
        (out / "files").mkdir(parents=True)
        reset(st, work)
        cmd = [str(dd), st["dmb"], str(a.port), "-trusted", "-invisible", "-ip", a.ip, "-verbose"]
        if not a.soak:
            cmd.append("-close")
        if st["params"]:
            cmd += ["-params", st["params"]]
        env = byond.env(bin_dir, **extra)
        print(f"[{label}] {' '.join(cmd)} {extra}", flush=True)
        t0 = time.time()
        limit = a.soak or a.timeout
        with open(out / "console.log", "wb") as log, open(out / "mem.csv", "w") as mcsv, \
                open(out / "topic.tsv", "w") as tsv:
            mcsv.write("secs,working_set_mb,private_mb,threads\n")
            p = subprocess.Popen(cmd, cwd=run_dir, env=env, stdout=log, stderr=subprocess.STDOUT)
            next_mem = next_topic = 0.0
            killed = False
            while p.poll() is None:
                el = time.time() - t0
                if el > limit:
                    p.kill()
                    killed = True
                    break
                if el >= next_mem:
                    m = mem(p.pid)
                    if m:
                        mcsv.write(f"{el:.0f},{m[0]},{m[1]},{m[2]}\n")
                        mcsv.flush()
                    next_mem = el + 5
                if a.topic and el >= next_topic and el > 20:
                    for q in a.topic:
                        dt, r = topic(a.ip, a.port, q)
                        tsv.write(f"{el:.0f}\t{q}\t{dt * 1000:.0f}\t{r!r:.300}\n")
                    tsv.flush()
                    next_topic = el + a.every
                time.sleep(0.5)
            rc = p.wait()
            rc = 128 - rc if rc < 0 else rc  # signal N -> 128+N, as byond.run
        secs = time.time() - t0
        time.sleep(1)
        collect(st, work, out / "files")
        res = summarize(out, rc, secs, killed, a.soak)
        res["env"], res["cmd"], res["dmb_sha"] = extra, cmd, st["dmb_sha"]
        (out / "summary.json").write_text(json.dumps(res, indent=1))
        t = res["tests"]
        print(f"[{label}] exit={rc} {'killed ' if killed else ''}{secs:.0f}s clean_run={res['clean_run']} "
              f"tests={collections.Counter(t.values()) if t else '-'} runtimes={res['runtimes_total']} "
              f"crash={res['crash']} peak_ws={res['mem'].get('peak_ws')}MB", flush=True)
    reset(st, work)


def runtime_key(line):
    """A runtime's message without what changes from run to run (time, log prefix, refs)."""
    line = re.sub(r"^\[[0-9:. -]+\]\s*(RUNTIME:\s*)?", "", line)
    line = re.sub(r"\[0x[0-9a-f]+\]", "[ref]", line)
    return line.strip()


def summarize(out, rc, secs, killed, soak):
    text = ANSI.sub("", (out / "console.log").read_bytes().decode("utf-8", "replace").replace("\r", ""))
    lines = text.splitlines()
    files = out / "files"
    tests = {}
    for f in files.rglob("unit_tests.json"):
        for k, v in json.loads(f.read_text(encoding="utf-8")).items():
            tests[k] = STAT.get(v.get("status"), str(v.get("status")))
    if not tests:  # no unit_tests.json (Paradise): the runner's "PASS|FAIL /datum/... 1.5s" lines
        for l in lines:
            m = re.match(r"\s*(PASS|FAIL|SKIP\w*)\s+(/datum/\S+)", l)
            if m:
                tests[m.group(2)] = {"PASS": "pass", "FAIL": "FAIL"}.get(m.group(1), "skip")
    # Runtimes from the console, or from the world's runtime logs (tgstation: data/logs/.../runtime.log)
    # when those have more: the same runtime can be in both.
    rt_re = re.compile(r"Runtime in |runtime error:")
    rt = collections.Counter(runtime_key(l) for l in lines if rt_re.search(l))
    for f in files.rglob("runtime*.log"):
        c = collections.Counter(runtime_key(l) for l in f.read_text("utf-8", "replace").splitlines()
                                if rt_re.search(l))
        if sum(c.values()) > sum(rt.values()):
            rt = c
    m = {}
    rows = [r.split(",") for r in (out / "mem.csv").read_text().splitlines()[1:]]
    if rows:
        ws = [int(r[1]) for r in rows]
        pv = [int(r[2]) for r in rows]
        m = {"samples": len(rows), "peak_ws": max(ws), "peak_private": max(pv), "last_ws": ws[-1],
             "last_private": pv[-1], "threads_max": max(int(r[3]) for r in rows)}
        if soak:  # growth over the second half of a soak, when the world is long up
            half = [r for r in rows if int(r[0]) >= int(rows[-1][0]) / 2]
            m["private_growth_2nd_half"] = int(half[-1][2]) - int(half[0][2])
    tp = out / "topic.tsv"
    topics = collections.Counter()
    if tp.exists():
        for r in tp.read_text().splitlines():
            q, reply = r.split("\t")[1], r.split("\t")[3]
            topics[f"{q} {'ok' if reply != 'None' else 'no reply'}"] += 1
    return {
        "exit": rc, "secs": round(secs), "killed": killed,
        "crash": (not killed and byond.crashed(rc)) or any(re.search(CRASH, l) for l in lines),
        "clean_run": any(files.rglob("clean_run.lk")),
        "test_run_failed": [l for l in lines if l.startswith(("Test run failed", "Total runtimes", "Unit Tests failed"))],
        "shutdown": any("Shutdown complete" in l for l in lines),
        "tests": tests,
        "runtimes_total": sum(rt.values()), "runtimes": dict(rt),
        "bug_lines": [l for l in lines if re.search(CRASH + r"|Out of memory", l)][:40],
        "dmfast_lines": byond.dmfast_lines(text)[:40],
        "mem": m, "topics": dict(topics),
    }


# --- compare --------------------------------------------------------------------------------------------

def compare(a):
    work = Path(a.work).resolve()
    labels = a.labels or sorted(p.name for p in (work / "out").iterdir() if (p / "summary.json").exists())
    r = {l: json.loads((work / "out" / l / "summary.json").read_text()) for l in labels}
    differ = False
    print("label        exit  secs  crash  clean  shutdown  tests(pass/FAIL/skip)  runtimes  peak_ws  peak_priv  topics")
    for l, s in r.items():
        c = collections.Counter(s["tests"].values())
        print(f"{l:12} {s['exit']:<5} {s['secs']:<5} {s['crash']!s:6} {s['clean_run']!s:6} {s['shutdown']!s:9} "
              f"{c['pass']}/{c['FAIL']}/{c['skip']:<16} {s['runtimes_total']:<9} {s['mem'].get('peak_ws', '-'):<8} "
              f"{s['mem'].get('peak_private', '-'):<10} {s['topics']}")
        if s["mem"].get("private_growth_2nd_half") is not None:
            print(f"{'':12} private bytes growth over the 2nd half: {s['mem']['private_growth_2nd_half']} MB")
    for k in ("exit", "crash", "clean_run", "shutdown", "test_run_failed"):
        if len({json.dumps(s[k]) for s in r.values()}) > 1:
            differ = True
            print(f"DIFF {k}: " + "  ".join(f"{l}={s[k]}" for l, s in r.items()))
    names = sorted(set().union(*(s["tests"] for s in r.values())))
    for n in names:
        v = [s["tests"].get(n, "missing") for s in r.values()]
        if len(set(v)) > 1:
            differ = True
            print(f"TEST {n}: " + "  ".join(f"{l}={x}" for l, x in zip(r, v)))
    fails = sorted({n for s in r.values() for n, x in s["tests"].items() if x == "FAIL"})
    if fails:
        print(f"failing in some run: {len(fails)}: {' '.join(fails)[:2000]}")
    msgs = sorted(set().union(*(s["runtimes"] for s in r.values())))
    for m in msgs:
        v = [s["runtimes"].get(m, 0) for s in r.values()]
        if len(set(v)) > 1:
            differ = True
            print(f"RUNTIME {' '.join(f'{l}={x}' for l, x in zip(r, v))}: {m[:300]}")
    for l, s in r.items():
        for b in s["bug_lines"][:10]:
            print(f"[{l}] {b[:200]}")
        for b in s["dmfast_lines"][:5]:
            print(f"[{l}] {b[:200]}")
    print("RESULTS DIFFER" if differ else "same results")
    return 1 if differ else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sp = ap.add_subparsers(dest="cmd", required=True)
    p = sp.add_parser("prep")
    p.add_argument("work")
    p.add_argument("--src", required=True)
    p.add_argument("--dme", required=True)
    p.add_argument("--bin", required=True)
    p.add_argument("-D", action="append", default=[])
    p.add_argument("--ci", metavar="MAP")
    p.add_argument("--lib", action="append", default=[])
    p.add_argument("--setup", metavar="BASH", help="command run in the run dir before its state is saved")
    p = sp.add_parser("run")
    p.add_argument("work")
    p.add_argument("--bin", required=True)
    p.add_argument("--ip", required=True)
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--mode", action="append")
    p.add_argument("--timeout", type=int, default=3600)
    p.add_argument("--soak", type=int, default=0)
    p.add_argument("--topic", action="append", default=[])
    p.add_argument("--every", type=int, default=10)
    p = sp.add_parser("compare")
    p.add_argument("work")
    p.add_argument("labels", nargs="*")
    a = ap.parse_args()
    if a.cmd == "run" and not a.mode:
        a.mode = ["stock", "default", "extended"]
    sys.exit({"prep": prep, "run": run, "compare": compare}[a.cmd](a) or 0)


if __name__ == "__main__":
    main()
