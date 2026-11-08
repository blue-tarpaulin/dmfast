# Test-only patch (seeker/tests/author.rs, and proof that the gate opens only in DreamSeeker): counts calls to
# byondcore.dll's DMString::DMString(), which every BYOND program calls constantly (410 direct calls inside
# byondcore.dll 516.1688; dreamseeker.exe and byondwin.dll import it).
#
# The export's bytes in 516.1688 (RVA 0x58730):
#   c7 01 00 00 00 00    mov dword ptr [ecx], 0
#   8b c1                mov eax, ecx
#   c3                   ret
# `displace 6` is exactly the first instruction: no relative operand, no base relocation inside it, and no branch in
# the DLL lands on its bytes 1 to 5. The body's `lock inc` changes flags, which no caller expects kept across a call.
patch counter byondcore.dll
about Counts DMString::DMString() calls in its data (test only).
tier safe
site export ??0DMString@@QAE@XZ
displace 6
body counter
data 4
