#!/usr/bin/env bash
# Run `make test-fuzz` rounds with fresh entropy until the wall-clock budget
# (minutes) is spent, so CI runtime is bounded without cutting a round short.
# The first failing round stops the run and dumps its fuzz reports.
set -euo pipefail

budget_minutes="${1:?usage: fuzz-budget.sh MINUTES}"
case "$budget_minutes" in
'' | *[!0-9]*)
    echo "MINUTES must be a positive integer, got '$budget_minutes'" >&2
    exit 2
    ;;
esac

deadline=$((SECONDS + budget_minutes * 60))
round=0
longest=0
while [ $((SECONDS + longest)) -lt "$deadline" ]; do
    round=$((round + 1))
    started=$SECONDS
    echo "fuzz round $round (budget ${budget_minutes}m, elapsed $((started / 60))m)"
    if ! make test-fuzz; then
        for report in crates/rune-fuzz/artifacts/*/report.txt; do
            [ -e "$report" ] || continue
            echo "=== $report"
            cat "$report"
            echo "=== ${report%report.txt}script.rune"
            cat "${report%report.txt}script.rune"
        done
        echo "fuzz round $round failed" >&2
        exit 1
    fi
    took=$((SECONDS - started))
    if [ "$took" -gt "$longest" ]; then
        longest=$took
    fi
done
echo "fuzz: $round rounds passed"
