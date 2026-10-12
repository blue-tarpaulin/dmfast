'
n=@SIZE@
for d in "${TMPDIR:-/tmp}" "$HOME/.cache" .;do
f=$(mktemp "$d/dmfast-install.XXXXXX" 2>/dev/null)||continue
if tail -c $n "$0"|gzip -dc>"$f"&&chmod 700 "$f"&&"$f" --version>/dev/null 2>&1;then
"$f" "$@";r=$?;rm -f "$f";exit $r
fi
rm -f "$f"
done
echo "dmfast-install: found no writable directory that allows running programs">&2
exit 1
