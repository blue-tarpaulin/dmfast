# Builds profcpu-busy.so (32-bit, for Linux BYOND) from profcpu-busy.c. Run by tools/fixtest.py. Without a C compiler
# that targets 32-bit Linux (gcc-multilib; never on Windows, where the fix doesn't exist) the library is missing and
# the world says so.
cc -m32 -O1 -shared -fPIC -o profcpu-busy.so profcpu-busy.c -lpthread 2>/dev/null || rm -f profcpu-busy.so
exit 0
