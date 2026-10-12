#!/usr/bin/env bash
# usage: compat.sh <BYOND version, e.g. 516.1688 or 516.latest> <dmfast-install.exe> <project.dme>
#
# Installs dmfast (with --runtime) into a fresh copy of that BYOND build from byond-builds.dm-lang.org
# and checks it against an untouched copy of the same build:
#   - compiling <project.dme> and tests/rtsmoke.dme gives byte-identical .dmb files;
#   - DreamDaemon running tests/rtsmoke.dmb logs the same result line.
# Prints which patch groups installed or disabled themselves. Appends a Markdown row to
# $GITHUB_STEP_SUMMARY when set and writes it to compat-<os>-<version>.md. Exits non-zero on any
# difference.
set -euo pipefail

version=$1; installer=$(realpath "$2"); project=$(realpath "$3")
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) os=windows; zip="${version}_byond.zip"; dm=dm.exe; dd=dd.exe ;;
  *) os=linux; zip="${version}_byond_linux.zip"; dm=DreamMaker; dd=DreamDaemon ;;
esac
# Paths handed to Windows programs.
native() { if [ "$os" = windows ]; then cygpath -w "$1"; else printf '%s\n' "$1"; fi; }

url="https://byond-builds.dm-lang.org/${version%%.*}/$zip"
if ! curl -sSfL -o "$work/byond.zip" "$url"; then
  # Client-only BYOND updates ship no Linux build.
  if [ "$os" = linux ] && [ "$(curl -s -o /dev/null -w '%{http_code}' "$url")" = 404 ]; then
    echo "== $version $os: skipped (no Linux build)"
    row="| $version | - | $os | no Linux build | | | | | |"
    echo "$row" > "compat-$os-$version.md"
    [ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$row" >> "$GITHUB_STEP_SUMMARY"
    exit 0
  fi
  echo "::error::$version $os: download failed ($url)"; exit 1
fi
mkdir -p "$work/x"
if [ "$os" = windows ]; then "$SYSTEMROOT/System32/tar.exe" -xf "$(native "$work/byond.zip")" -C "$(native "$work/x")"
else unzip -q -d "$work/x" "$work/byond.zip"; fi
cp -r "$work/x/byond/bin" "$work/stock"
cp -r "$work/x/byond/bin" "$work/dmfast"
if [ "$os" = windows ]; then "$installer" --runtime "$(native "$work/dmfast")" >"$work/install.txt" 2>&1
else sh "$installer" --runtime "$work/dmfast" >"$work/install.txt" 2>&1; fi || { cat "$work/install.txt"; exit 1; }

# run <bin dir> <program> <args...>: run a BYOND program from that build with dmfast's stats on.
run() {
  local bin=$1 prog=$2; shift 2
  if [ "$os" = linux ]; then LD_LIBRARY_PATH="$bin" DMFAST_STATS=1 "$bin/$prog" "$@"
  else DMFAST_STATS=1 "$bin/$prog" "$@"; fi
}

fail=0
# compile <bin dir> <dme>: sets $secs and $hash (sha256 of the .dmb), returns non-zero if the compile
# fails; the output is in $work/out.txt.
compile() {
  local start end
  rm -f "${2%.dme}.dmb"
  start=$(date +%s.%N)
  (cd "$(dirname "$2")" && run "$1" "$dm" "$(basename "$2")") >"$work/out.txt" 2>&1 || return 1
  end=$(date +%s.%N)
  secs=$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.1f", b-a}')
  hash=$(sha256sum "${2%.dme}.dmb" | cut -c1-16)
}

# A project may refuse some BYOND builds itself (minimum/maximum version checks): then only rtsmoke runs.
if compile "$work/stock" "$project"; then
  t_stock=${secs}s; h_stock=$hash
  if compile "$work/dmfast" "$project"; then
    t_fast=${secs}s; h_fast=$hash
    [ "$h_stock" = "$h_fast" ] || { echo "::error::$version $os: $(basename "$project") .dmb differs"; fail=1; }
  else
    echo "::error::$version $os: $(basename "$project") compiles with stock but not with dmfast"; cat "$work/out.txt"; fail=1
  fi
  compiler=$(grep -E '^dmfast: ' "$work/out.txt" || true)
else
  echo "stock BYOND $version does not compile $(basename "$project"); testing rtsmoke only:"
  grep -iE 'error' "$work/out.txt" | head -3 || true
  t_stock="n/a"; t_fast="n/a"
fi

compile "$work/stock" "$root/tests/rtsmoke.dme" || { echo "::error::$version $os: stock rtsmoke compile failed"; cat "$work/out.txt"; exit 1; }
s_stock=$hash
compile "$work/dmfast" "$root/tests/rtsmoke.dme" || { echo "::error::$version $os: dmfast rtsmoke compile failed"; cat "$work/out.txt"; exit 1; }
s_fast=$hash
[ -n "${compiler:-}" ] || compiler=$(grep -E '^dmfast: ' "$work/out.txt" || true)
[ "$s_stock" = "$s_fast" ] || { echo "::error::$version $os: rtsmoke .dmb differs"; fail=1; }

# daemon <bin dir>: sets $dd_rc (DreamDaemon's exit code) and $dd_line (the world's result line, if any).
daemon() {
  local log="$root/tests/rtsmoke.log" env=(DMFAST_STATS=1)
  [ "$os" = linux ] && env+=(LD_LIBRARY_PATH="$1")
  rm -f "$log"
  dd_rc=0
  (cd "$root/tests" && env "${env[@]}" timeout 120 "$1/$dd" rtsmoke.dmb -trusted -invisible -logself >"$work/dd.txt" 2>&1) || dd_rc=$?
  dd_line=$({ grep -h 'len=' "$log" 2>/dev/null || true; } | sed 's/^\[[^]]*\] *//' | tail -1)
}
daemon "$work/stock"; d_stock=$dd_line
daemon "$work/dmfast"; d_fast=$dd_line; fast_rc=$dd_rc
runtime=$(grep -E '^dmfast: ' "$work/dd.txt" || true)
if [ -n "$d_stock" ]; then
  [ "$d_stock" = "$d_fast" ] || { echo "::error::$version $os: DreamDaemon result differs ('$d_stock' vs '$d_fast')"; fail=1; }
else
  # Some builds (around 516.1660-1665) print nothing from world/New even on stock: then only check
  # that dmfast's DreamDaemon exits cleanly.
  echo "stock DreamDaemon $version printed no result; checking dmfast exits cleanly (exit $fast_rc)"
  [ "$fast_rc" = 0 ] || { echo "::error::$version $os: DreamDaemon with dmfast exited $fast_rc"; fail=1; }
fi

disabled=$(printf '%s\n%s\n' "$compiler" "$runtime" | grep ' disabled: ' | sed 's/^dmfast: //; s/ disabled:.*//' | paste -sd, - || true)
mask() { printf '%s\n' "$1" | grep -oE 'patched=0x[0-9a-f]+' | tail -1 | cut -d= -f2; }
real=$(run "$work/stock" "$dm" 2>&1 | grep -oE '[0-9]{3}\.[0-9]{4}' | head -1 || true)
result=$([ $fail = 0 ] && echo pass || echo FAIL)

echo "== $version ($real) $os: $result"
echo "compile $(basename "$project"): stock $t_stock, dmfast $t_fast, .dmb ${h_stock:-n/a} / ${h_fast:-n/a}"
echo "compiler groups: $(mask "$compiler"); runtime groups: $(mask "$runtime"); disabled: ${disabled:-none}"
printf '%s\n' "$compiler" "$runtime"

row="| $version | ${real:-?} | $os | $result | $t_stock | $t_fast | $(mask "$compiler") | $(mask "$runtime") | ${disabled:-none} |"
echo "$row" > "compat-$os-$version.md"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$row" >> "$GITHUB_STEP_SUMMARY"
exit $fail
