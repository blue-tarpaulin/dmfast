# Writes varlimit-pad.dm: padding variable declarations (each distinct name takes a compiler id) so that the
# `tail` procs' local variables, declared after them, are numbered across the compiler's reserved ids at 65484+
# (whatever this BYOND build's own starting count is). Run by tools/fixtest.py.
{
  :
  for ((i = 0; i < 65200; i++)); do ((i % 50 == 0)) && echo "/datum/holder$((i / 50))"; echo "	var/p$i"; done
} > varlimit-pad.dm
{
  for ((i = 0; i < 500; i++)); do
    printf '/proc/tail%d()\n\tvar/tv%d = %d\n\treturn tv%d\n' $i $i $i $i
  done
} > varlimit-tail.dm
{
  echo "/world/New()"
  echo "	var/sum = 0"
  for ((i = 0; i < 500; i++)); do echo "	sum += tail$i()"; done
  echo '	world.log << "sum [sum]"'
  echo "	shutdown()"
} > varlimit-main.dm
