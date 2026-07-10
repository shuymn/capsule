#!/usr/bin/env bash

set -euo pipefail

for required_command in git git-cliff; do
	if ! command -v "${required_command}" >/dev/null 2>&1; then
		echo "required command is not installed: ${required_command}" >&2
		exit 1
	fi
done

bump="${1:-}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

version="$(bash "${script_dir}/version.sh" bump "${bump}")"
tag="v${version}"

git-cliff \
	--config cliff.toml \
	--unreleased \
	--tag "${tag}" \
	--prepend crates/cli/CHANGELOG.md

bash "${script_dir}/check.sh" HEAD >/dev/null
printf '%s\n' "${version}"
