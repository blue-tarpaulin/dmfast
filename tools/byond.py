"""Running a BYOND build's programs; shared by compat.py, fixtest.py and dmtests.py."""
import hashlib, os, platform, shutil, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WIN = platform.system() == "Windows"
OS = "windows" if WIN else "linux"
DM, DD = ("dm.exe", "dd.exe") if WIN else ("DreamMaker", "DreamDaemon")
DM_ORIG, DD_ORIG = ("dm_orig.exe", "dd_orig.exe") if WIN else ("DreamMaker_orig", "DreamDaemon_orig")
TIMEOUT = 124  # exit code of a run that hit its time limit (as timeout(1))


def env(bin_dir, **vars):
    """The environment for a program of the BYOND build in bin_dir: DMFAST_FIXES only when given, and no
    RUST_MIN_STACK (a Rust build setting; inherited, it sets the stack of every thread a world's Rust library
    starts, and 8 MiB stacks run a 32-bit DreamDaemon out of address space when rust-g runs hundreds of jobs)."""
    e = {k: v for k, v in os.environ.items() if k not in ("DMFAST_FIXES", "RUST_MIN_STACK")}
    if not WIN:
        e["LD_LIBRARY_PATH"] = str(bin_dir)
    e.update(vars)
    return e


def run(cmd, cwd, env, limit):
    """(exit code, output). Signals become 128 + signal; TIMEOUT on time limit."""
    try:
        p = subprocess.run([str(c) for c in cmd], cwd=cwd, env=env, timeout=limit,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        rc, out = (128 - p.returncode if p.returncode < 0 else p.returncode), p.stdout
    except subprocess.TimeoutExpired as e:
        rc, out = TIMEOUT, e.stdout or b""
    return rc, out.decode("utf-8", "replace").replace("\r", "")


def crashed(rc):
    """A run's exit code means the process died (signal on Linux, NTSTATUS exception on Windows)."""
    return rc in (134, 139) or rc >= 0xC0000000 or (WIN and rc == 3)


def world_lines(text):
    """What a world printed: the lines after BYOND's banner ("Welcome BYOND!"), without blank lines and
    dmfast's own."""
    lines = text.replace("\r", "").splitlines()
    i = next((n for n, l in enumerate(lines) if "Welcome BYOND" in l), None)
    return [] if i is None else [l for l in lines[i + 1:] if l.strip() and not l.startswith("dmfast")]


def dmfast_lines(text):
    return [l for l in text.replace("\r", "").splitlines() if l.startswith("dmfast: ")]


def sha16(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()[:16]


def bins(fast, stock=None):
    """(fast dm, fast dd, stock dm, stock dd); stock defaults to the _orig programs beside dmfast."""
    fast = Path(fast).resolve()
    if stock:
        stock = Path(stock).resolve()
        return fast / DM, fast / DD, stock / DM, stock / DD
    return fast / DM, fast / DD, fast / DM_ORIG, fast / DD_ORIG


def bash():
    """A bash for the tests' generator scripts: Git's on Windows (System32's is WSL's)."""
    git = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git" / "bin" / "bash.exe"
    return str(git) if WIN and git.exists() else shutil.which("bash") or "bash"
