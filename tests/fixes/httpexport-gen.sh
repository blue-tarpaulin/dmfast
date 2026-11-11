#!/bin/bash
# 128 MB to download and, on Windows (the fix is Windows-only), a server for it on a fresh port (in
# httpexport-port.txt), run from outside this folder so the test can delete it.
head -c 134217728 /dev/urandom > httpexport.bin
port=$((40000 + RANDOM % 20000))
echo "$port" > httpexport-port.txt
if [ "$OS" = Windows_NT ]; then
  here=$(cygpath -w "$PWD")
  (cd /tmp && powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$here\\httpexport-serve.ps1" \
    "$here\\httpexport.bin" "$port" > /dev/null 2>&1 &)
  sleep 2
fi
