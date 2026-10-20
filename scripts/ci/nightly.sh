#!/usr/bin/env bash
# Nightly gate state: the tag `nightly-ok` marks the last commit whose full
# nightly run passed. `changed` prints true/false; `mark` moves the tag to HEAD,
# and only a HEAD that is part of the remote `main` history may be marked.
set -euo pipefail

marker=nightly-ok
head_sha="$(git rev-parse HEAD)"

case "${1:?usage: nightly.sh changed|mark}" in
changed)
    # An unreachable remote is an error, never "unchanged".
    remote="$(git ls-remote --tags origin "refs/tags/$marker")"
    tested="${remote%%[[:space:]]*}"
    if [ "$tested" = "$head_sha" ]; then
        echo false
    else
        echo true
    fi
    ;;
mark)
    git fetch --quiet origin main
    if ! git merge-base --is-ancestor "$head_sha" FETCH_HEAD; then
        echo "refusing to mark $head_sha: not in the history of origin/main" >&2
        exit 1
    fi
    git tag --force "$marker" "$head_sha"
    git push --force origin "refs/tags/$marker"
    ;;
*)
    echo "usage: nightly.sh changed|mark" >&2
    exit 2
    ;;
esac
