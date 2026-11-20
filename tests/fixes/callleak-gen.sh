# Builds the call_ext() library callleak.dme calls from callleak-lib.rs, with rustc for 32-bit Windows or Linux.
# Run by tools/fixtest.py.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) target=i686-pc-windows-msvc out=callleak.dll ;;
*) target=i686-unknown-linux-gnu out=libcallleak.so ;;
esac
rustc --edition 2021 --crate-type cdylib --target $target -O -o $out callleak-lib.rs
