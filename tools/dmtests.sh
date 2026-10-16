#!/usr/bin/env bash
# usage: dmtests.sh [-s <stock bin dir>] [-f <fixes>] [-j <jobs>] <dmfast bin dir> [filter]
#
# Runs the DM test suite (tests/dm-test-suite, spacestation13/dm-test-suite) with stock BYOND and with dmfast
# and compares them test by test. Each test is one .dm with /proc/RunTest; its first line may tag it
# "// COMPILE ERROR" or "// RUNTIME ERROR" (expected to fail that way; a suffix such as "OD" after it is
# the suite's own note), "// IGNORE" / "// NOBYOND" (skipped).
# Per test, each side gets a verdict: pass, compile (didn't compile), runtime (runtime error or output),
# crash (DreamDaemon died or hung), or expected-* when a tagged test did not fail as tagged; with the .dmb's
# hash when it compiled.
#
# Reported: tests whose verdict differs between stock and dmfast (a dmfast bug; a test where stock BYOND crashes
# and dmfast doesn't is a notice: dmfast's allocator doesn't trip on BYOND's memory bug), tests whose .dmb differs with
# the fixes off (compile fixes may change it: pass -f none to check identity; a .dmb that differs between two
# stock compiles, DEBUG's build stamp, is not compared), and the suite's own result on
# each side (tests BYOND itself fails are listed, not counted against dmfast). Exits non-zero only on a
# stock/dmfast difference. -f sets DMFAST_FIXES for the dmfast side (default none); -j runs that many tests at
# once (default 4); [filter] is a substring of the test path.
#
# <dmfast bin dir> has dmfast installed. Stock comes from <stock bin dir>, or the _orig binaries the installer
# kept in <dmfast bin dir>.
set -euo pipefail

stock=""; fixes=none; jobs=4
while [ $# -gt 0 ]; do
  case $1 in
    -s) stock=$2; shift 2 ;;
    -f) fixes=$2; shift 2 ;;
    -j) jobs=$2; shift 2 ;;
    *) break ;;
  esac
done
[ $# -ge 1 ] || { sed -n '2p' "$0" | cut -c3-; exit 2; }
fast=$(cd "$1" && pwd); filter=${2:-}
root=$(cd "$(dirname "$0")/.." && pwd)
suite="$root/tests/dm-test-suite/Tests"
[ -d "$suite/Tests" ] || { echo "tests/dm-test-suite is empty: git submodule update --init"; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) os=windows; dm=dm.exe; dd=dd.exe; sdm=dm_orig.exe; sdd=dd_orig.exe ;;
  *) os=linux; dm=DreamMaker; dd=DreamDaemon; sdm=DreamMaker_orig; sdd=DreamDaemon_orig ;;
esac
if [ -n "$stock" ]; then stock=$(cd "$stock" && pwd); sdm=$dm; sdd=$dd; else stock=$fast; fi
hash() { sha256sum "$1" | cut -c1-16; }

# verdict <bin> <dm> <dd> <fixes> <test.dm> <workdir>: writes "<verdict> <dmb hash or ->" to <workdir>/verdict
verdict() {
  mkdir -p "$6"; echo "crash -" >"$6/verdict"   # replaced below; stays if anything here fails
  verdict_ "$@"
}
verdict_() {
  local bin=$1 prog_dm=$2 prog_dd=$3 fx=$4 test=$5 t=$6 tag v="" h=-
  # As the suite's own runner: the tag is a prefix; "COMPILE ERROR OD..." (also tracked for OpenDream) counts.
  tag=$({ head -n 2 "$test" | grep -oE '^// ?(COMPILE ERROR|RUNTIME ERROR|IGNORE|NOBYOND)' || true; } | head -1 | tr -d '/' | sed 's/^ *//')
  case $tag in IGNORE|NOBYOND) echo "skip -" >"$t/verdict"; return ;; esac
  rm -rf "$t"; mkdir -p "$t"; cp -r "$suite"/. "$t/"
  local rel; rel=$(realpath --relative-to="$suite" "$test")
  # The suite's dm.sh: the test and the crash wrapper are #included on the .dme's third line.
  { sed -n '1,2p' "$t/environment.dme"; printf '#include "%s"\n#include "crashwrapper.dm"\n' "$rel"; sed -n '3,$p' "$t/environment.dme"; } >"$t/t.dme"
  local env=(DMFAST_FIXES="$fx")
  [ "$os" = linux ] && env+=(LD_LIBRARY_PATH="$bin")
  (cd "$t" && env "${env[@]}" timeout 120 "$bin/$prog_dm" -max_errors 0 t.dme) >"$t/dm.txt" 2>&1 || true
  if ! grep -q -- '- 0 errors, ' "$t/dm.txt" || [ ! -f "$t/t.dmb" ]; then
    v=$([ "$tag" = "COMPILE ERROR" ] && echo pass || echo compile)
    echo "$v -" >"$t/verdict"; return
  fi
  h=$(hash "$t/t.dmb")
  if [ "$tag" = "COMPILE ERROR" ]; then echo "expected-compile $h" >"$t/verdict"; return; fi
  local rc=0
  (cd "$t" && env "${env[@]}" timeout 60 "$bin/$prog_dd" t.dmb -once -close -trusted -invisible -log errors.log -ip 127.0.0.1) >"$t/dd.txt" 2>&1 || rc=$?
  if [ $rc != 0 ]; then echo "crash $h" >"$t/verdict"; return; fi
  # errors.log: BYOND's banner up to "Welcome BYOND!", then anything the world printed (world.log, runtime
  # errors). The banner is 3 lines on Linux and 5 on Windows.
  local errs; errs=$(sed '1,/^Welcome BYOND!/d; /^[[:space:]]*$/d' "$t/errors.log" 2>/dev/null | tr -d '\r' | grep -v '^dmfast: ' || true)
  if [ -n "$errs" ]; then v=$([ "$tag" = "RUNTIME ERROR" ] && echo pass || echo runtime)
  else v=$([ "$tag" = "RUNTIME ERROR" ] && echo expected-runtime || echo pass); fi
  echo "$v $h" >"$t/verdict"
}

# one <n> <test>: both sides of one test into $work/r/<n>. The suite builds with DEBUG, which stamps the
# .dmb with the build time: when the hashes differ, stock is compiled again and a hash that moved on its own
# is reported as "~" (not comparable) instead of a difference.
one() {
  local n=$1 test=$2 s f
  verdict "$stock" "$sdm" "$sdd" none "$test" "$work/t/$n/s"
  verdict "$fast" "$dm" "$dd" "$fixes" "$test" "$work/t/$n/f"
  s=$(cat "$work/t/$n/s/verdict"); f=$(cat "$work/t/$n/f/verdict")
  if [ "${s#* }" != "${f#* }" ] && [ "${s#* }" != - ] && [ "${f#* }" != - ]; then
    verdict "$stock" "$sdm" "$sdd" none "$test" "$work/t/$n/s2"
    [ "$(cat "$work/t/$n/s2/verdict")" = "$s" ] || { s="${s%% *} ~"; f="${f%% *} ~"; }
  fi
  printf '%s\t%s\t%s\n' "$(realpath --relative-to="$suite" "$test")" "$s" "$f" >"$work/r/$n"
  if [ "${s%% *}" != "${f%% *}" ]; then   # keep what each side printed, for the report
    for side in s f; do
      { echo "--- $([ $side = s ] && echo stock || echo dmfast) compiler:"; grep -E '(error|warning):' "$work/t/$n/$side/dm.txt" || true
        echo "--- world:"; sed '1,/^Welcome BYOND!/d; /^[[:space:]]*$/d' "$work/t/$n/$side/errors.log" 2>/dev/null || true; } | head -40
    done >"$work/r/$n.detail" 2>&1
  fi
  rm -rf "$work/t/$n"
}

mkdir -p "$work/r" "$work/t"
mapfile -t tests < <(find "$suite/Tests" -type f -name "*.dm" -path "*$filter*" | sort)
[ ${#tests[@]} -gt 0 ] || { echo "dmtests: no tests match '$filter'"; exit 2; }
n=0
for test in "${tests[@]}"; do
  n=$((n + 1))
  one "$n" "$test" &
  while [ "$(jobs -rp | wc -l)" -ge "$jobs" ]; do sleep 0.2; done
done
wait

find "$work/r" -type f ! -name '*.detail' -exec cat {} + | sort >"$work/all.tsv"
total=$(grep -cv $'\tskip' "$work/all.tsv" || true)
diffs=$(awk -F'\t' '{ split($2, s, " "); split($3, f, " "); if (s[1] != f[1] && s[1] != "crash") print }' "$work/all.tsv")
masked=$(awk -F'\t' '{ split($2, s, " "); split($3, f, " "); if (s[1] == "crash" && f[1] != "crash") print $1 " (dmfast: " f[1] ")" }' "$work/all.tsv")
dmbdiff=$(awk -F'\t' '{ split($2, s, " "); split($3, f, " "); if (s[2] != "-" && f[2] != "-" && s[2] != f[2]) print $1 }' "$work/all.tsv")
sfail=$(awk -F'\t' '{ split($2, s, " "); if (s[1] != "pass" && s[1] != "skip") print $1 " (" s[1] ")" }' "$work/all.tsv")
ffail=$(awk -F'\t' '{ split($3, f, " "); if (f[1] != "pass" && f[1] != "skip") print $1 " (" f[1] ")" }' "$work/all.tsv")

if [ -n "$diffs" ]; then
  echo "::error::dmtests: verdict differs between stock and dmfast (test, stock, dmfast):"
  printf '%s\n' "$diffs" | sed 's/^/  /'
  for d in "$work"/r/*.detail; do [ -e "$d" ] && { echo "== $(head -1 "${d%.detail}" | cut -f1)"; cat "$d"; }; done
fi
if [ -n "$masked" ]; then
  echo "::notice::dmtests: stock BYOND crashes, dmfast does not (its allocator doesn't trip on BYOND's memory bug):"
  printf '%s\n' "$masked" | sed 's/^/  /'
fi
if [ -n "$dmbdiff" ]; then
  [ "$fixes" = none ] && lvl=error || lvl=notice
  echo "::$lvl::dmtests: .dmb differs from stock (DMFAST_FIXES=$fixes):"; printf '%s\n' "$dmbdiff" | sed 's/^/  /'
fi
[ -z "$sfail" ] || { echo "stock BYOND fails:"; printf '%s\n' "$sfail" | sed 's/^/  /'; }
[ -z "$ffail" ] || { echo "dmfast (DMFAST_FIXES=$fixes) fails:"; printf '%s\n' "$ffail" | sed 's/^/  /'; }
nd=$(printf '%s' "$diffs" | grep -c . || true); nb=$(printf '%s' "$dmbdiff" | grep -c . || true)
ns=$(printf '%s' "$sfail" | grep -c . || true); nf=$(printf '%s' "$ffail" | grep -c . || true)
echo "dmtests: $total tests, $nd verdict differences, $nb .dmb differences; stock fails $ns, dmfast fails $nf"
[ "$nd" = 0 ] && { [ "$fixes" != none ] || [ "$nb" = 0 ]; }
