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

curl -sSfL -o "$work/byond.zip" "https://byond-builds.dm-lang.org/${version%%.*}/$zip"
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
# compile <bin dir> <dme>: sets $secs and $hash (sha256 of the .dmb); the output is in $work/out.txt.
compile() {
  local start end
  rm -f "${2%.dme}.dmb"
  start=$(date +%s.%N)
  (cd "$(dirname "$2")" && run "$1" "$dm" "$(basename "$2")") >"$work/out.txt" 2>&1 || { echo "compile failed ($1, $2)" >&2; cat "$work/out.txt" >&2; exit 1; }
  end=$(date +%s.%N)
  secs=$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.1f", b-a}')
  hash=$(sha256sum "${2%.dme}.dmb" | cut -c1-16)
}

compile "$work/stock" "$project"; t_stock=$secs; h_stock=$hash
compile "$work/dmfast" "$project"; t_fast=$secs; h_fast=$hash
compiler=$(grep -E '^dmfast: ' "$work/out.txt" || true)
[ "$h_stock" = "$h_fast" ] || { echo "::error::$version $os: $(basename "$project") .dmb differs"; fail=1; }

compile "$work/stock" "$root/tests/rtsmoke.dme"; s_stock=$hash
compile "$work/dmfast" "$root/tests/rtsmoke.dme"; s_fast=$hash
[ "$s_stock" = "$s_fast" ] || { echo "::error::$version $os: rtsmoke .dmb differs"; fail=1; }

daemon() { # daemon <bin dir> -> the world's result line
  local log="$root/tests/rtsmoke.log" env=(DMFAST_STATS=1)
  [ "$os" = linux ] && env+=(LD_LIBRARY_PATH="$1")
  rm -f "$log"
  (cd "$root/tests" && env "${env[@]}" timeout 120 "$1/$dd" rtsmoke.dmb -trusted -invisible -logself >"$work/dd.txt" 2>&1) || true
  grep -h 'len=' "$log" 2>/dev/null | sed 's/^\[[^]]*\] *//' | tail -1
}
d_stock=$(daemon "$work/stock")
d_fast=$(daemon "$work/dmfast")
runtime=$(grep -E '^dmfast: ' "$work/dd.txt" || true)
[ -n "$d_stock" ] && [ "$d_stock" = "$d_fast" ] || { echo "::error::$version $os: DreamDaemon result differs ('$d_stock' vs '$d_fast')"; fail=1; }

disabled=$(printf '%s\n%s\n' "$compiler" "$runtime" | grep ' disabled: ' | sed 's/^dmfast: //; s/ disabled:.*//' | paste -sd, - || true)
mask() { printf '%s\n' "$1" | grep -oE 'patched=0x[0-9a-f]+' | tail -1 | cut -d= -f2; }
real=$(run "$work/stock" "$dm" 2>&1 | grep -oE '[0-9]{3}\.[0-9]{4}' | head -1 || true)
result=$([ $fail = 0 ] && echo pass || echo FAIL)

echo "== $version ($real) $os: $result"
echo "compile: stock ${t_stock}s, dmfast ${t_fast}s, .dmb $h_stock / $h_fast"
echo "compiler groups: $(mask "$compiler"); runtime groups: $(mask "$runtime"); disabled: ${disabled:-none}"
printf '%s\n' "$compiler" "$runtime"

row="| $version | ${real:-?} | $os | $result | ${t_stock}s | ${t_fast}s | $(mask "$compiler") | $(mask "$runtime") | ${disabled:-none} |"
echo "$row" > "compat-$os-$version.md"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$row" >> "$GITHUB_STEP_SUMMARY"
exit $fail
