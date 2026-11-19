# Builds the call_ext() library callextleak.dme calls from callextleak-lib.rs, with rustc for 32-bit Windows or Linux.
# Run by tools/fixtest.py.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) target=i686-pc-windows-msvc out=callextleak.dll ;;
*) target=i686-unknown-linux-gnu out=libcallextleak.so ;;
esac
rustc --edition 2021 --crate-type cdylib --target $target -O -o $out callextleak-lib.rs
