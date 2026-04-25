#!/usr/bin/env bash
set -euo pipefail

condition="${1:-proof-full}"
workspace="workspace/${condition}"

if [[ ! -d "$workspace" ]]; then
    echo "missing workspace: $workspace" >&2
    exit 1
fi

if [[ -d "$workspace/.git" ]]; then
    git -C "$workspace" reset --hard --quiet
    git -C "$workspace" clean -fdx --quiet
else
    git -C "$workspace" init --quiet
    git -C "$workspace" add .
    git -C "$workspace" -c user.name=quorp -c user.email=quorp@example.com commit -qm "Challenge baseline"
fi

cp START_HERE.md "$workspace/START_HERE.md"
cp SUCCESS.md "$workspace/SUCCESS.md"
cp benchmark.json "$workspace/benchmark.json"
rm -rf "$workspace/.quorp"
