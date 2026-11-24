#!/usr/bin/env bash
# usage: offline-test.sh <dmfast bin dir>      (Linux; strace needed)
#
# Test for `--offline` (the `offline` fix): DreamDaemon started with it doesn't connect to the BYOND hub, and the
# world's own connections still work. <dmfast bin dir> has BYOND's programs and libbyond.so with libdmfast.so beside
# them. Checks, each printed with ok or WRONG:
#   - stock (no libdmfast.so) refuses the unknown option;
#   - with dmfast, the world starts, a world.Export() to itself answers, and strace sees no connection to the hub's
#     port (20002, or 6001 when invisible);
#   - without --offline, dmfast connects to the hub as stock does (the check above can see one).
# Prints "offline: pass" (exit 0) or "offline: FAIL" (exit 1).
set -u
bin=$(cd "${1:?usage: offline-test.sh <dmfast bin dir>}" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
unset RUST_MIN_STACK
cd "$work"
cat > t.dme <<'DM'
/world/New()
	world.log << "export: [world.Export("byond://127.0.0.1:[world.port]?ping")]"
	spawn(50) shutdown()
/world/Topic()
	return "pong"
DM
LD_LIBRARY_PATH=$bin "$bin/DreamMaker" t.dme >/dev/null 2>&1 || { echo "offline: FAIL (can't compile)"; exit 1; }

fail=0
check() { # check <label> <ok?>
  if [ "$2" = 1 ]; then st=ok; else st=WRONG; fail=1; fi
  printf '%-48s %s\n' "$1" "$st"
}
# run <args...>: dmfast's DreamDaemon's output in $out, the number of hub connections strace saw in $hub.
run() {
  out=$(LD_LIBRARY_PATH=$bin timeout 40 strace -f -qq -o trace.txt -e trace=connect -e signal=none \
    -E LD_PRELOAD="$bin/libdmfast.so" "$bin/DreamDaemon" t.dmb 54830 -ip 127.0.0.1 "$@" 2>&1)
  hub=$(grep -cE 'htons\((20002|6001)\)' trace.txt)
}

out=$(LD_LIBRARY_PATH=$bin timeout 20 "$bin/DreamDaemon" t.dmb 54830 --offline 2>&1)
check "stock refuses --offline" "$([[ $out == *"invalid option '--offline'"* ]] && echo 1)"
run --offline
check "dmfast --offline: world runs, Export answers" "$([[ $out == *"export: pong"* ]] && echo 1)"
check "dmfast --offline: no hub connection ($hub)" "$([ "$hub" = 0 ] && echo 1)"
run
check "dmfast without --offline: hub connection ($hub)" "$([ "$hub" -gt 0 ] && echo 1)"
if [ $fail = 0 ]; then echo "offline: pass"; else echo "offline: FAIL"; exit 1; fi
