#!/usr/bin/env bash
# usage: compat.sh <BYOND version, e.g. 516.1688 or 516.latest> <dmfast-install.exe> <project.dme>
#
# Installs dmfast (with --runtime) into a fresh copy of that BYOND build from byond-builds.dm-lang.org
# and checks it against an untouched copy of the same build:
#   - compiling <project.dme> and tests/rtsmoke.dme gives byte-identical .dmb files;
#   - DreamDaemon running tests/rtsmoke.dmb logs the same result line;
#   - in verify mode (DMFAST_VERIFY=1) no patch disagrees with the original code.
# Also times the compile and how long DreamDaemon takes to load the project's world (until the world
# prints its first line), stock vs dmfast.
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
    row="| $version | - | $os | no Linux build | | | | | | | | |"
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
mkdir -p "$work/fast" && cp -r "$work/x/byond/bin" "$work/fast/bin"
if [ "$os" = windows ]; then "$installer" --runtime "$(native "$work/fast/bin")" >"$work/install.txt" 2>&1
else sh "$installer" --runtime "$work/fast/bin" >"$work/install.txt" 2>&1; fi || { cat "$work/install.txt"; exit 1; }

# run <bin dir> <program> <args...>: run a BYOND program from that build with dmfast's stats on, for at
# most $limit seconds (default 120; exit status 124 when it hit the limit).
run() {
  local bin=$1 prog=$2; shift 2
  if [ "$os" = linux ]; then LD_LIBRARY_PATH="$bin" DMFAST_STATS=1 timeout "${limit:-120}" "$bin/$prog" "$@"
  else DMFAST_STATS=1 timeout "${limit:-120}" "$bin/$prog" "$@"; fi
}

# broken <why>: stock BYOND itself doesn't work on this runner (some builds hang or crash); record the
# build as broken and pass, since there is nothing to compare dmfast against.
broken() {
  echo "::warning::$version $os: stock BYOND build looks broken ($1); skipping"
  row="| $version | ${real:-?} | $os | broken build: $1 | | | | | | | | |"
  echo "$row" > "compat-$os-$version.md"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$row" >> "$GITHUB_STEP_SUMMARY"
  exit 0
}

fail=0
# compile <bin dir> <dme>: sets $secs and $hash (sha256 of the .dmb); returns the compiler's exit status
# (124 if it ran out of time). The output is in $work/out.txt.
compile() {
  local start end rc=0
  rm -f "${2%.dme}.dmb"
  start=$(date +%s.%N)
  (cd "$(dirname "$2")" && run "$1" "$dm" "$(basename "$2")") >"$work/out.txt" 2>&1 || rc=$?
  [ "$rc" = 0 ] || return "$rc"
  end=$(date +%s.%N)
  secs=$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.1f", b-a}')
  hash=$(sha256sum "${2%.dme}.dmb" | cut -c1-16)
}

# A project may refuse some BYOND builds itself (minimum/maximum version checks): then only rtsmoke runs.
rc=0; compile "$work/stock" "$project" || rc=$?
[ "$rc" != 124 ] || broken "$(basename "$project") compile timed out"
if [ "$rc" = 0 ]; then
  t_stock=${secs}s; h_stock=$hash
  rc=0; compile "$work/fast/bin" "$project" || rc=$?
  if [ "$rc" = 0 ]; then
    t_fast=${secs}s; h_fast=$hash
    [ "$h_stock" = "$h_fast" ] || { echo "::error::$version $os: $(basename "$project") .dmb differs"; fail=1; }
  else
    echo "::error::$version $os: $(basename "$project") compiles with stock but not with dmfast (exit $rc)"; cat "$work/out.txt"; fail=1
  fi
  compiler=$(grep -E '^dmfast: ' "$work/out.txt" || true)
else
  echo "stock BYOND $version does not compile $(basename "$project"); testing rtsmoke only:"
  grep -iE 'error' "$work/out.txt" | head -3 || true
  t_stock="n/a"; t_fast="n/a"
fi

rc=0; compile "$work/stock" "$root/tests/rtsmoke.dme" || rc=$?
[ "$rc" != 124 ] || broken "rtsmoke compile timed out"
[ "$rc" = 0 ] || broken "rtsmoke compile failed (exit $rc)"
s_stock=$hash
compile "$work/fast/bin" "$root/tests/rtsmoke.dme" || { echo "::error::$version $os: dmfast rtsmoke compile failed"; cat "$work/out.txt"; exit 1; }
s_fast=$hash
[ -n "${compiler:-}" ] || compiler=$(grep -E '^dmfast: ' "$work/out.txt" || true)
[ "$s_stock" = "$s_fast" ] || { echo "::error::$version $os: rtsmoke .dmb differs"; fail=1; }

# daemon <bin dir>: sets $dd_rc (DreamDaemon's exit code) and $dd_line (the world's result line, if any).
daemon() {
  local log="$root/tests/rtsmoke.log" env=(DMFAST_STATS=1)
  [ "$os" = linux ] && env+=(LD_LIBRARY_PATH="$1")
  rm -f "$log"
  dd_rc=0
  # -ip 127.0.0.1: listen on loopback only, so Windows doesn't ask for a firewall rule for every fresh copy.
  (cd "$root/tests" && env "${env[@]}" timeout 120 "$1/$dd" rtsmoke.dmb -trusted -invisible -logself -ip 127.0.0.1 >"$work/dd.txt" 2>&1) || dd_rc=$?
  dd_line=$({ grep -h 'len=' "$log" 2>/dev/null || true; } | sed 's/^\[[^]]*\] *//' | tail -1)
}
daemon "$work/stock"; d_stock=$dd_line
[ "$dd_rc" != 124 ] || broken "DreamDaemon timed out on rtsmoke"
daemon "$work/fast/bin"; d_fast=$dd_line; fast_rc=$dd_rc
runtime=$(grep -E '^dmfast: ' "$work/dd.txt" || true)
if [ -n "$d_stock" ]; then
  [ "$d_stock" = "$d_fast" ] || { echo "::error::$version $os: DreamDaemon result differs ('$d_stock' vs '$d_fast')"; fail=1; }
else
  # Some builds (around 516.1660-1665) print nothing from world/New even on stock: then only check
  # that dmfast's DreamDaemon exits cleanly.
  echo "stock DreamDaemon $version printed no result; checking dmfast exits cleanly (exit $fast_rc)"
  [ "$fast_rc" = 0 ] || { echo "::error::$version $os: DreamDaemon with dmfast exited $fast_rc"; fail=1; }
fi

# Verify mode: every patch that has one also works out each answer the original way and counts
# disagreements (DMFAST_STATS lines end in mismatches=N). Run on the project's compile, rtsmoke's compile
# and rtsmoke in DreamDaemon; the .dmb files must still match stock.
vlog="$work/verify.txt"; : >"$vlog"
export DMFAST_VERIFY=1
limit=300 # verify mode works most answers out twice
if [ -n "${h_fast:-}" ]; then
  if compile "$work/fast/bin" "$project"; then
    cat "$work/out.txt" >>"$vlog"
    [ "$hash" = "$h_stock" ] || { echo "::error::$version $os: $(basename "$project") .dmb differs in verify mode"; fail=1; }
  else
    echo "::error::$version $os: $(basename "$project") verify-mode compile failed"; cat "$work/out.txt"; fail=1
  fi
fi
if compile "$work/fast/bin" "$root/tests/rtsmoke.dme"; then
  cat "$work/out.txt" >>"$vlog"
  [ "$hash" = "$s_stock" ] || { echo "::error::$version $os: rtsmoke .dmb differs in verify mode"; fail=1; }
else
  echo "::error::$version $os: rtsmoke verify-mode compile failed"; cat "$work/out.txt"; fail=1
fi
daemon "$work/fast/bin"; cat "$work/dd.txt" >>"$vlog"
unset DMFAST_VERIFY limit
checked=$({ grep -E '^dmfast: .*mismatches=' "$vlog" || true; } | wc -l | tr -d ' ')
bad=$(grep -E '^dmfast: .*mismatches=[1-9]' "$vlog" || true)
if [ -n "$bad" ]; then
  echo "::error::$version $os: verify mode found mismatches"; printf '%s\n' "$bad"; fail=1
  verify="FAIL"
else
  verify="ok ($checked checks)"
fi

# load_secs <bin dir> <dmb>: prints seconds until DreamDaemon's world prints its first line (after BYOND's
# banner), or nothing if it doesn't within 60 s. The world is stopped then.
load_secs() {
  local out="$work/load.txt" start pid elapsed="" deadline
  : >"$out"
  start=$(date +%s.%N); deadline=$(( $(date +%s) + 60 ))
  if [ "$os" = linux ]; then
    (cd "$(dirname "$2")" && exec env LD_LIBRARY_PATH="$1" "$1/$dd" "$(basename "$2")" -trusted -invisible -ip 127.0.0.1) >"$out" 2>&1 &
  else
    (cd "$(dirname "$2")" && exec "$1/$dd" "$(basename "$2")" -trusted -invisible -ip 127.0.0.1) >"$out" 2>&1 &
  fi
  pid=$!
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if sed -n '/Welcome BYOND/,$p' "$out" | tail -n +2 | grep -v '^dmfast' | grep -q '[^[:space:]]'; then
      elapsed=$(awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}'); break
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.05
  done
  if [ "$os" = windows ]; then taskkill //F //T //PID "$(cat "/proc/$pid/winpid")" >/dev/null 2>&1 || true
  else kill "$pid" 2>/dev/null || true; fi
  wait "$pid" 2>/dev/null || true
  printf '%s\n' "$elapsed"
}
median() { printf '%s\n' "$@" | { grep . || true; } | sort -g | awk '{v[NR]=$1} END{if (NR) printf "%ss", v[int((NR+1)/2)]}'; }
l_stock=""; l_fast=""
if [ -n "${h_fast:-}" ]; then
  # Some builds' worlds never print anything (as with rtsmoke above): then there is nothing to time.
  first=$(load_secs "$work/stock" "${project%.dme}.dmb")
  if [ -n "$first" ]; then
    ls=("$first"); lf=()
    for _ in 1 2 3; do
      lf+=("$(load_secs "$work/fast/bin" "${project%.dme}.dmb")")
      [ ${#ls[@]} = 3 ] || ls+=("$(load_secs "$work/stock" "${project%.dme}.dmb")")
    done
    l_stock=$(median "${ls[@]}"); l_fast=$(median "${lf[@]}")
  else
    echo "stock DreamDaemon $version printed nothing from $(basename "$project")'s world in 60 s; not timing world load"
  fi
fi
disabled=$(printf '%s\n%s\n' "$compiler" "$runtime" | grep ' disabled: ' | sed 's/^dmfast: //; s/ disabled:.*//' | paste -sd, - || true)
mask() { printf '%s\n' "$1" | grep -oE 'patched=0x[0-9a-f]+' | tail -1 | cut -d= -f2; }
real=$(run "$work/stock" "$dm" 2>&1 | grep -oE '[0-9]{3}\.[0-9]{4}' | head -1 || true)
result=$([ $fail = 0 ] && echo pass || echo FAIL)

echo "== $version ($real) $os: $result"
echo "compile $(basename "$project"): stock $t_stock, dmfast $t_fast, .dmb ${h_stock:-n/a} / ${h_fast:-n/a}"
echo "world load: stock ${l_stock:-n/a}, dmfast ${l_fast:-n/a}"
echo "verify mode: $verify"
echo "compiler groups: $(mask "$compiler"); runtime groups: $(mask "$runtime"); disabled: ${disabled:-none}"
printf '%s\n' "$compiler" "$runtime"

row="| $version | ${real:-?} | $os | $result | $t_stock | $t_fast | $(mask "$compiler") | $(mask "$runtime") | ${disabled:-none} | ${l_stock:-n/a} | ${l_fast:-n/a} | $verify |"
echo "$row" > "compat-$os-$version.md"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$row" >> "$GITHUB_STEP_SUMMARY"
exit $fail
