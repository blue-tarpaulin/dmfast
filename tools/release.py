"""Build the combined Windows + Linux installer: dist/dmfast-install.exe.

usage: py tools/release.py [--distro NAME]

Run on Windows with WSL. Builds the Windows installer natively and the Linux installer inside WSL (needs
Rust with the i686-unknown-linux-gnu target and 32-bit glibc dev files on a system like Debian 12, whose glibc 2.36
binds nothing past BYOND's floor of 2.34; see README), then packs both into one file with dmfast-pack. The result is a
normal Windows executable that Linux runs with `sh dmfast-install.exe <bin dir>`.
"""
import argparse
import os
import shlex
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST = os.path.join(ROOT, 'dist')
# Linux build directory inside WSL (much faster than on the Windows drive).
WSL_TARGET = '~/.cache/dmfast-release-target'


def run(cmd, **kw):
    print('>', ' '.join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=ROOT, **kw)
    if r.returncode:
        sys.exit(f'release: {cmd[0]} failed with exit code {r.returncode}')
    return r


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--distro', help='WSL distribution to build the Linux installer in (default: WSL default)')
    a = ap.parse_args()
    wsl = ['wsl'] + (['-d', a.distro] if a.distro else []) + ['--']
    os.makedirs(DIST, exist_ok=True)

    run(['cargo', 'build', '--release', '-p', 'dmfast-install'])
    win = os.path.join(ROOT, 'target', 'release', 'dmfast-install.exe')

    root_wsl = run(wsl + ['wslpath', '-a', ROOT.replace('\\', '/')], capture_output=True, text=True).stdout.strip()
    linux = os.path.join(DIST, 'dmfast-install-linux')
    # No `$VAR`s in the command: wsl.exe expands those in its arguments before bash sees them.
    script = (
        'set -e; source ~/.cargo/env 2>/dev/null || true; '
        f'cd {shlex.quote(root_wsl)}; '
        f'CARGO_TARGET_DIR={WSL_TARGET} cargo build --release -p dmfast-install; '
        f'cp {WSL_TARGET}/release/dmfast-install {shlex.quote(root_wsl + "/dist/dmfast-install-linux")}'
    )
    run(wsl + ['bash', '-c', script])

    out = os.path.join(DIST, 'dmfast-install.exe')
    run(['cargo', 'run', '-q', '--release', '-p', 'dmfast-pack', '--', win, linux, out])
    os.remove(linux)


if __name__ == '__main__':
    main()
