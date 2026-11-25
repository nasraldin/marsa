#!/usr/bin/env bash
# Only a nonempty, known documentation diff may skip Tier 1 build jobs.
set -euo pipefail

if [[ -z "${1:-}" || -z "${2:-}" ]]; then
    echo false
    exit 0
fi

paths="$(mktemp)"
trap 'rm -f -- "$paths"' EXIT

# A rename is a deletion and an addition, so both paths are checked.
if ! git diff --name-only --no-renames -z "$1" "$2" -- > "$paths"; then
    echo false
    exit 0
fi

docs_only=false
while IFS= read -r -d '' path; do
    case "$path" in
        docs/*.md|AGENTS.md|CODE_OF_CONDUCT.md|CONTRIBUTING.md|README.md|SECURITY.md)
            docs_only=true
            ;;
        *)
            echo false
            exit 0
            ;;
    esac
done < "$paths"

echo "$docs_only"
