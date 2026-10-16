#!/usr/bin/env bash
# usage: fixtest.sh [-s <stock bin dir>] <dmfast bin dir> [key...]
#
# Tests dmfast's opt-in bug fixes (DMFAST_FIXES; see BUGFIXES.md). Each fix <key> has a test world,
# tests/fixes/<key>.dme, and the output it must give with the fix on, tests/fixes/<key>.expected. The test
# is compiled and run (DreamDaemon) with:
#   - stock BYOND: must NOT give the expected output (the test reproduces the bug);
#   - dmfast with DMFAST_FIXES=<key>: must give it;
#   - dmfast with DMFAST_FIXES=breaking (every fix): must give it too (no fix breaks another).
# A world's output is: the compiler's error and warning lines (prefixed "dm: "), "dm: no .dmb" if it
# didn't compile, then everything the world logged after BYOND's banner (world.log, runtime errors)
# except blank lines, dmfast's lines and what BYOND prints once a world is up (its web client address and
# the date), then "dd: exit N" or "dd: timed out" unless DreamDaemon exited cleanly within 60 s. The
# world should log its results and call shutdown().
#
# <dmfast bin dir> has dmfast installed (with DreamDaemon, the default). The stock programs are taken from <stock bin dir>,
# or else the stock binaries the installer kept in <dmfast bin dir> (dm_orig.exe, DreamMaker_orig, ...).
# With no keys, every test in tests/fixes runs. A fix that didn't install in this BYOND build (its code
# didn't match) is reported as n/a, not as a failure; so is a build whose worlds print nothing at all, and
# stock already giving the expected output (BYOND fixed the bug itself), with a warning. Ends with a line
# "fixes: <summary>"; exits non-zero if any test failed.
#
# tests/fixes/keep/<name>.dme are regression worlds: code that must behave as on stock BYOND with a fix tier
# on (patterns real codebases use that a fix could reach). Each runs on stock and with DMFAST_FIXES=safe and
# =extended; the three outputs must be identical (stock is the oracle, there is no .expected). They run with
# no keys or with the key "keep", after the fix tests, and count as one result each in the summary.
set -euo pipefail

stock=""
if [ "${1:-}" = -s ]; then stock=$2; shift 2; fi
[ $# -ge 1 ] || { sed -n '2,3p' "$0" | cut -c3-; exit 2; }
fast=$(cd "$1" && pwd); shift
root=$(cd "$(dirname "$0")/.." && pwd)
tests="$root/tests/fixes"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) os=windows; dm=dm.exe; dd=dd.exe; sdm=dm_orig.exe; sdd=dd_orig.exe ;;
  *) os=linux; dm=DreamMaker; dd=DreamDaemon; sdm=DreamMaker_orig; sdd=DreamDaemon_orig ;;
esac
if [ -n "$stock" ]; then stock=$(cd "$stock" && pwd); sdm=$dm; sdd=$dd; else stock=$fast; fi
native() { if [ "$os" = windows ]; then cygpath -w "$1"; else printf '%s\n' "$1"; fi; }

keep=0
if [ $# -gt 0 ]; then
  keys=()
  for k in "$@"; do if [ "$k" = keep ]; then keep=1; else keys+=("$k"); fi; done
else
  keep=1
  keys=(); for f in "$tests"/*.dme; do [ -e "$f" ] && keys+=("$(basename "$f" .dme)"); done
fi
if [ ${#keys[@]} = 0 ] && [ $keep = 0 ]; then echo "fixes: none"; exit 0; fi

# out <bin dir> <dm> <dd> <fixes> <key> <file>: compiles and runs tests/fixes/<key>.dme in $work/t with
# DMFAST_FIXES=<fixes> and writes its output (see above) to <file>, dmfast's own lines to <file>.dmfast.
out() {
  local bin=$1 prog_dm=$2 prog_dd=$3 fixes=$4 key=$5 file=$6 t="$work/t" rc=0
  local env=(DMFAST_STATS=1 DMFAST_FIXES="$fixes")
  [ "$os" = linux ] && env+=(LD_LIBRARY_PATH="$bin")
  rm -rf "$t"; mkdir -p "$t"; cp "$tests/$key".* "$t/"; rm -f "$t/$key.expected"
  for f in "$tests/$key"-*; do if [ -e "$f" ]; then cp -r "$f" "$t/"; fi; done
  if [ -f "$t/$key-gen.sh" ]; then (cd "$t" && bash "./$key-gen.sh"); fi
  (cd "$t" && env "${env[@]}" timeout 120 "$bin/$prog_dm" "$key.dme") >"$work/dm.txt" 2>&1 || true
  {
    tr -d '\r' <"$work/dm.txt" | { grep -v '^dmfast' || true; } | { grep -E '(error|warning):' || true; } | sed 's/^/dm: /'
    if [ -f "$t/$key.dmb" ]; then
      # -ip 127.0.0.1: loopback only, so Windows doesn't ask for a firewall rule for every fresh copy.
      (cd "$t" && env "${env[@]}" timeout 60 "$bin/$prog_dd" "$key.dmb" -trusted -invisible -logself -ip 127.0.0.1) \
        >"$work/dd.txt" 2>&1 || rc=$?
      { tr -d '\r' <"$t/$key.log" 2>/dev/null || true; } | sed -n '/Welcome BYOND/,$p' | tail -n +2 \
        | { grep -vE '^(dmfast|Webclient users can visit|http://www\.byond\.com/play/|Or this code can be embedded:|<iframe src="http://www\.byond\.com/play/|The BYOND hub reports that port [0-9]+ is not reachable)|^((Mon|Tue|Wed|Thu|Fri|Sat|Sun) [A-Z][a-z]{2} [ 0-9]{2} [0-9:]{8} [0-9]{4})?[[:space:]]*$' || true; }
      case $rc in 0) ;; 124) echo "dd: timed out" ;; *) echo "dd: exit $rc" ;; esac
    else
      echo "dm: no .dmb"
    fi
  } | P1="$bin" P2="$(native "$bin")" P3="$t" P4="$(native "$t")" awk '{ for (k = 1; k <= 4; k++) { p = ENVIRON["P" k]
        while (p != "" && (i = index($0, p)) > 0) $0 = substr($0, 1, i - 1) "#dir" substr($0, i + length(p)) } } 1' >"$file"
  { cat "$work/dm.txt" "$work/dd.txt" "$t/$key.log" 2>/dev/null || true; } | tr -d '\r' | { grep -E '^dmfast: ' || true; } >"$file.dmfast"
  rm -f "$work/dd.txt"
}

# Some builds (around 516.1660-1665) print nothing from any world. Find out with a world that only logs a
# line: on such a build, a test whose fixed run printed nothing at all has nothing to compare.
mkdir -p "$work/probe"; printf '/world/New()\n\tworld.log << "probe"\n\tshutdown()\n' >"$work/probe/probe.dme"
real_tests=$tests; tests="$work/probe"; out "$stock" "$sdm" "$sdd" "" probe "$work/probe.txt"; tests=$real_tests
silent=$([ -s "$work/probe.txt" ] && echo 0 || echo 1)

pass=0; failed=0; na=0; result=()
for key in "${keys[@]}"; do
  if [ ! -f "$tests/$key.dme" ]; then
    echo "::error::fix $key: no tests/fixes/$key.dme"; failed=$((failed + 1)); result+=("$key FAIL"); continue
  fi
  out "$fast" "$dm" "$dd" "$key" "$key" "$work/fixed.txt"
  if ! grep -qE "^dmfast: fix $key: " "$work/fixed.txt.dmfast"; then
    if grep -qx 'dm: no .dmb' "$work/fixed.txt"; then
      # A DreamDaemon fix can't have installed if the test world never ran.
      echo "::error::fix $key: the test didn't compile with DMFAST_FIXES=$key:"; cat "$work/fixed.txt"
      failed=$((failed + 1)); result+=("$key FAIL"); continue
    fi
    why=$(grep -E "^dmfast: disabled fix $key - " "$work/fixed.txt.dmfast" | head -1 | sed "s/^dmfast: disabled fix $key - //" || true)
    echo "fix $key: n/a (didn't install${why:+: $why})"
    na=$((na + 1)); result+=("$key n/a"); continue
  fi
  out "$stock" "$sdm" "$sdd" "" "$key" "$work/stock.txt"
  out "$fast" "$dm" "$dd" breaking "$key" "$work/all.txt"
  if [ ! -f "$tests/$key.expected" ]; then
    echo "::error::fix $key: no tests/fixes/$key.expected. Stock gave:"; cat "$work/stock.txt"
    echo "With the fix:"; cat "$work/fixed.txt"
    failed=$((failed + 1)); result+=("$key FAIL"); continue
  fi
  tr -d '\r' <"$tests/$key.expected" >"$work/expected.txt"
  bad=0
  if [ "$silent" = 1 ] && [ ! -s "$work/fixed.txt" ]; then
    echo "fix $key: n/a (worlds print nothing on this BYOND build)"
    na=$((na + 1)); result+=("$key n/a"); continue
  fi
  for run in fixed all; do
    if ! cmp -s "$work/expected.txt" "$work/$run.txt"; then
      echo "::error::fix $key: wrong output with DMFAST_FIXES=$([ $run = fixed ] && echo "$key" || echo breaking) (< expected, > got):"
      diff "$work/expected.txt" "$work/$run.txt" | head -20 || true
      bad=1
    fi
  done
  if [ $bad = 1 ]; then failed=$((failed + 1)); result+=("$key FAIL"); continue; fi
  if cmp -s "$work/expected.txt" "$work/stock.txt"; then
    echo "::warning::fix $key: stock BYOND already gives the expected output (fixed upstream, or the test doesn't reproduce the bug)"
    na=$((na + 1)); result+=("$key not needed"); continue
  fi
  echo "fix $key: pass"
  pass=$((pass + 1)); result+=("$key ok")
done

# Regression worlds: stock, safe and extended must agree.
if [ $keep = 1 ] && [ -d "$real_tests/keep" ]; then
  for f in "$real_tests"/keep/*.dme; do
    [ -e "$f" ] || continue
    name=$(basename "$f" .dme); tests="$real_tests/keep"
    out "$stock" "$sdm" "$sdd" "" "$name" "$work/k-stock.txt"
    bad=0
    for tier in safe extended; do
      out "$fast" "$dm" "$dd" "$tier" "$name" "$work/k-$tier.txt"
      if ! cmp -s "$work/k-stock.txt" "$work/k-$tier.txt"; then
        echo "::error::keep $name: output differs from stock with DMFAST_FIXES=$tier (< stock, > $tier):"
        diff "$work/k-stock.txt" "$work/k-$tier.txt" | head -20 || true
        bad=1
      fi
    done
    tests=$real_tests
    if [ $bad = 1 ]; then failed=$((failed + 1)); result+=("keep/$name FAIL"); continue; fi
    if [ ! -s "$work/k-stock.txt" ]; then
      if [ "$silent" = 1 ]; then na=$((na + 1)); result+=("keep/$name n/a"); continue; fi
      echo "::error::keep $name: the world printed nothing"; failed=$((failed + 1)); result+=("keep/$name FAIL"); continue
    fi
    echo "keep $name: same as stock"
    pass=$((pass + 1)); result+=("keep/$name ok")
  done
fi

summary="$pass pass"
[ $failed = 0 ] || summary+=", $failed FAIL"
[ $na = 0 ] || summary+=", $na n/a"
echo "fixes: $summary ($(IFS=,; echo "${result[*]}" | sed 's/,/, /g'))"
[ $failed = 0 ]
