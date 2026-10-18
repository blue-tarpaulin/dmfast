#!/usr/bin/env python3
"""Boots a tgstation world on dmfast until its subsystems finish initializing, and a bit past it.

usage: boot.py [--limit SECS] [--after SECS] <dmfast bin dir> <tgstation.dme>

Compiles the project with dmfast, starts DreamDaemon (the default map, MetaStation) and waits for
"Initializations complete". Fails if the world exits or crashes before that, doesn't get there within --limit,
or exits within --after seconds after it. Runtime errors are only reported (a ::warning:: with the first
ones): tgstation's random station traits and map can cause some on stock BYOND too.
"""
import argparse, re, shutil, subprocess, sys, time
from pathlib import Path

import byond

DONE = re.compile(r"Initializations complete within ([\d.]+) seconds?")


def log(*a):
    print(*a, flush=True)


def runtimes(project, output):
    """Runtime error lines of this boot: tgstation's runtime logs under data/logs (its world.log echoes some of
    them), or the world's output if it wrote none."""
    logs = sorted((project.parent / "data" / "logs").rglob("runtime.log"))
    text = "\n".join(f.read_text("utf-8", "replace") for f in logs) if logs else output
    return [l for l in text.splitlines() if "runtime error:" in l]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bin_dir", type=Path)
    ap.add_argument("project", type=Path)
    ap.add_argument("--limit", type=int, default=900, help="seconds to reach Initializations complete")
    ap.add_argument("--after", type=int, default=60, help="seconds the world must keep running after it")
    a = ap.parse_args()
    bin_dir, project = a.bin_dir.resolve(), a.project.resolve()

    t = time.monotonic()
    rc, out = byond.run([bin_dir / byond.DM, project.name], project.parent, byond.env(bin_dir), 300)
    if rc != 0:
        log(out[-3000:])
        sys.exit(f"::error::{project.name} doesn't compile with dmfast (exit {rc})")
    log(f"compiled in {time.monotonic() - t:.0f} s")

    dmb = project.with_suffix(".dmb")
    shutil.rmtree(project.parent / "data" / "logs", ignore_errors=True)  # count only this boot's runtimes
    raw = project.parent / "boot-output.txt"
    env = byond.env(bin_dir, DMFAST_STATS="1")
    cmd = [str(bin_dir / byond.DD), dmb.name, "-trusted", "-invisible", "-ip", "127.0.0.1"]
    with open(raw, "wb") as f:
        p = subprocess.Popen(cmd, cwd=project.parent, env=env, stdout=f, stderr=subprocess.STDOUT)
        start, done, deadline = time.monotonic(), None, time.monotonic() + a.limit
        while p.poll() is None and time.monotonic() < deadline:
            if done is None and (m := DONE.search(raw.read_text("utf-8", "replace"))):
                done = time.monotonic() - start
                log(f"Initializations complete after {done:.0f} s (the world says {m.group(1)} s)")
                deadline = time.monotonic() + a.after
            time.sleep(1)
        rc = p.poll()
        if rc is None:
            if byond.WIN:
                subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)], capture_output=True)
            else:  # SIGKILL: tgstation catches SIGTERM and takes its time shutting down
                p.kill()
            p.wait()
    output = raw.read_text("utf-8", "replace").replace("\r", "")
    log("\n".join(byond.dmfast_lines(output)))

    errors = runtimes(project, output)
    if errors:
        log(f"::warning::{len(errors)} runtime error(s) during the boot (tgstation can cause some on stock too):")
        log("\n".join(errors[:20]))

    tail = "\n".join(output.splitlines()[-40:])
    if rc is not None:
        how = "crashed" if byond.crashed(rc) else "exited"
        when = "before Initializations complete" if done is None else f"within {a.after} s after Initializations complete"
        log(tail)
        sys.exit(f"::error::the world {how} (exit {rc}) {when}")
    if done is None:
        log(tail)
        sys.exit(f"::error::no Initializations complete within {a.limit} s")
    log(f"boot ok: initialized in {done:.0f} s, still running {a.after} s later, {len(errors)} runtime error(s)")


if __name__ == "__main__":
    main()
