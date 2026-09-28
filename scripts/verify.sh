#!/usr/bin/env bash
# Runs the full test suite in Docker and writes a report to
# reports/<timestamp>-report.md (also copied to reports/latest.md).
# Exit code: 0 if everything passed, 1 otherwise.
#
# Usage: scripts/verify.sh [extra docker build args...]
set -uo pipefail
cd "$(dirname "$0")/.."

mkdir -p reports
stamp=$(date +%Y%m%d-%H%M%S)
raw="reports/$stamp-build.log"
log="reports/$stamp-build.clean.log"
report="reports/$stamp-report.md"

DOCKER_BUILDKIT=1 docker build --target test --progress=plain "$@" . >"$raw" 2>&1
status=$?

# strip BuildKit's "#12 34.5 " line prefixes so patterns can match
sed -E 's/^#[0-9]+ [0-9]+\.[0-9]+ //' "$raw" >"$log"

section() { # title, command...
  local title=$1; shift
  local out
  out=$("$@" 2>/dev/null)
  if [ -n "$out" ]; then
    printf '\n## %s\n\n```\n%s\n```\n' "$title" "$out"
  fi
}

{
  echo "# Verification report $stamp"
  echo
  echo "- Result: **$([ $status -eq 0 ] && echo PASS || echo FAIL)**"
  echo "- Command: \`docker build --target test --progress=plain . $*\`"
  echo "- Commit: $(git rev-parse --short HEAD 2>/dev/null || echo none)," \
       "uncommitted files: $(git status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
  echo "- Raw log: $log"

  section "Compile errors" grep -E -A12 '^error(\[E[0-9]+\])?:' "$log"
  section "Test results" grep -E '^test .* \.\.\. |^test result:|^running [0-9]+ tests|Running (unittests|tests/)' "$log"
  section "Failure details" awk '/^failures:$/{p=1} p{print} /^test result:/{p=0}' "$log"
  section "Warnings (first 40 lines)" bash -c "grep -E -A6 '^warning: ' '$log' | head -40"
  section "Docker/build errors" grep -E 'ERROR: |failed to solve|did not complete successfully' "$log"
  section "Log tail" tail -40 "$log"
} >"$report"

cp "$report" reports/latest.md
echo "report: $report"
exit $status
