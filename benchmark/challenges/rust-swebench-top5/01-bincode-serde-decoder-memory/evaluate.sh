#!/usr/bin/env bash
set -euo pipefail

condition="${1:-proof-full}"
workspace="workspace/${condition}"

if [[ ! -d "$workspace" ]]; then
    echo "missing workspace: $workspace" >&2
    exit 1
fi

(
    cd "$workspace"
    cargo test --quiet --features serde --test issues issue_474
)
