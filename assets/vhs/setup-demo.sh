#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

docker build \
    --file "$repo_root/assets/vhs/Dockerfile" \
    --tag capsule-vhs-demo \
    "$repo_root"
