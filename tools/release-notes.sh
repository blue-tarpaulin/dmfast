#!/usr/bin/env bash
# usage: release-notes.sh <dir with compat-*.md rows> [project name]
#
# Prints the release notes body: tagline, the 516.latest speedups the release's own compatibility test
# measured on GitHub's runners (compile and world load, Windows and Linux), install steps and how to
# verify the download. The release job appends GitHub's generated changelog.
set -euo pipefail

dir=$1; project=${2:-tgstation}

# field <os> <column>: a column of the 516.latest row for that OS (columns as written by compat.sh).
field() { awk -F'|' -v os="$1" -v c="$2" '$2 ~ /516\.latest/ && $4 ~ os {gsub(/[ s]/, "", $c); print $c}' "$dir"/compat-*.md | head -1; }
# speedup <stock secs> <dmfast secs>: "6.5x faster", or "-" if either is missing.
speedup() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a + 0 > 0 && b + 0 > 0) printf "**%.1fx faster**", a / b; else printf "-" }'; }

build=$(field linux 3); [ -n "$build" ] && [ "$build" != "-" ] || build=$(field windows 3)

cat <<EOF
Robusts BYOND's compiler.

### $project on BYOND $build

| | Windows | Linux |
|---|---|---|
| Compile | $(speedup "$(field windows 6)" "$(field windows 7)") | $(speedup "$(field linux 6)" "$(field linux 7)") |
| DreamDaemon world load (\`--runtime\`) | $(speedup "$(field windows 11)" "$(field windows 12)") | $(speedup "$(field linux 11)" "$(field linux 12)") |

Measured on this release's GitHub Actions runners; your gains depend on your computer and codebase.

### Install

1. Close BYOND.
2. Download \`dmfast-install.exe\` below.
3. Run it on your BYOND \`bin\` folder (or drag \`dm.exe\` onto it). Add \`--runtime\` to also speed up DreamDaemon.

On Linux: \`sh dmfast-install.exe ~/byond/bin\`

### Verify

\`\`\`
gh attestation verify dmfast-install.exe -R ${GITHUB_REPOSITORY:-blue-tarpaulin/dmfast}
\`\`\`
EOF
