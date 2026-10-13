#!/usr/bin/env bash
# usage: release-notes.sh <dir with compat-*.md rows> [project name]
#
# Prints the release notes body: tagline, the release's section of CHANGELOG.md (`## <version>`, the tag
# without its `v`, from GITHUB_REF_NAME), the 516.latest speedups the release's own compatibility test
# measured on GitHub's runners (compile and world load, Windows and Linux), install steps and how to
# verify the download. The release job appends GitHub's generated changelog.
set -euo pipefail

dir=$1; project=${2:-tgstation}
root=$(cd "$(dirname "$0")/.." && pwd)
version=${GITHUB_REF_NAME:-}; version=${version#v}

# The CHANGELOG.md section headed `## <version>` (anything may follow the version on that line), without
# leading or trailing blank lines; empty if there is none.
changes=$(awk -v v="$version" '
  /^## / { on = v != "" && ($2 == v || $2 == "v" v); next }
  on { lines[++n] = $0 }
  END { a = 1; while (a <= n && lines[a] ~ /^[ \t]*$/) a++; b = n; while (b >= a && lines[b] ~ /^[ \t]*$/) b--; for (i = a; i <= b; i++) print lines[i] }
' "$root/CHANGELOG.md")
if [ -n "$version" ] && [ -z "$changes" ]; then
  echo "::warning::CHANGELOG.md has no '## $version' section; the release notes have no changelog text" >&2
fi

# field <os> <column>: a column of the 516.latest row for that OS (columns as written by compat.sh).
field() { awk -F'|' -v os="$1" -v c="$2" '$2 ~ /516\.latest/ && $4 ~ os {gsub(/[ s]/, "", $c); print $c}' "$dir"/compat-*.md | head -1; }
# speedup <stock secs> <dmfast secs>: "6.5x faster", or "-" if either is missing.
speedup() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a + 0 > 0 && b + 0 > 0) printf "**%.1fx faster**", a / b; else printf "-" }'; }

build=$(field linux 3); [ -n "$build" ] && [ "$build" != "-" ] || build=$(field windows 3)

echo "Robusts BYOND's toolchain."
[ -z "$changes" ] || printf '\n### %s Changes\n\n%s\n' "$version" "$changes"

cat <<EOF

### $project on BYOND $build

| | Windows | Linux |
|---|---|---|
| Compile | $(speedup "$(field windows 6)" "$(field windows 7)") | $(speedup "$(field linux 6)" "$(field linux 7)") |
| DreamDaemon world load | $(speedup "$(field windows 11)" "$(field windows 12)") | $(speedup "$(field linux 11)" "$(field linux 12)") |

Automatically measured on this release's GitHub Actions runners; your gains depend on your computer and codebase.

### Install

1. Close BYOND.
2. Download \`dmfast-install.exe\` below.
3. Run it on your BYOND \`bin\` folder (or drag \`dm.exe\` onto it). It speeds up the compiler and DreamDaemon; add \`--no-runtime\` to leave DreamDaemon stock.

On Linux: \`sh dmfast-install.exe ~/byond/bin\`

### Verify

\`\`\`
gh attestation verify dmfast-install.exe -R ${GITHUB_REPOSITORY:-blue-tarpaulin/dmfast}
\`\`\`
EOF
