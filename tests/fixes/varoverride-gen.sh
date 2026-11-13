# Writes varoverride-pad.dm: padding var names (each distinct name takes a string id), so that the names of the 300
# vars in varoverride-vars.dm, declared after them, get string ids across the interpreter's built-in ids 0xFFCD-0xFFF1
# (whatever this BYOND build's own starting count is). Run by tools/fixtest.py.
{
	for ((i = 0; i < 64900; i++)); do ((i % 100 == 0)) && echo "/datum/pad$((i / 100))"; echo "	var/p$i"; done
} > varoverride-pad.dm
{
	echo "/datum/parent"
	for ((i = 0; i < 300; i++)); do echo "	var/list/v$i = list(\"parent\")"; done
	echo "/datum/parent/nulls"
	for ((i = 0; i < 300; i++)); do echo "	v$i = null"; done
	echo "/datum/parent/lists"
	for ((i = 0; i < 300; i++)); do echo "	v$i = list(\"child\")"; done
} > varoverride-vars.dm
{
	echo "/proc/check(datum/parent/d, want)"
	echo "	var/n = 0"
	for ((i = 0; i < 300; i++)); do echo "	if(d.v$i ~= want) n++"; done
	echo "	return n"
	echo "/proc/check_initial(datum/parent/d, want)"
	echo "	var/n = 0"
	for ((i = 0; i < 300; i++)); do echo "	if(initial(d.v$i) ~= want) n++"; done
	echo "	return n"
} > varoverride-check.dm
