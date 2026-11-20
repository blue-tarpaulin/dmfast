# Builds the call_ext() library await.dme calls from callleak-lib.rs (in the tests dir), with rustc for 32-bit Windows or Linux.
# Run by tools/fixtest.py.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) target=i686-pc-windows-msvc out=await.dll ;;
*) target=i686-unknown-linux-gnu out=libawait.so ;;
esac
rustc --edition 2021 --crate-type cdylib --target $target -O -o $out "$1/callleak-lib.rs"
