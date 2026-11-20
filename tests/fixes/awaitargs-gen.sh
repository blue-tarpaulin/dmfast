# Builds the call_ext() library awaitargs.dme calls from callextleak-lib.rs (in the tests dir, $1), with rustc for
# 32-bit Windows or Linux. Run by tools/fixtest.py.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) target=i686-pc-windows-msvc out=awaitargs.dll ;;
*) target=i686-unknown-linux-gnu out=libawaitargs.so ;;
esac
rustc --edition 2021 --crate-type cdylib --target $target -O -o $out "$1/callextleak-lib.rs"
