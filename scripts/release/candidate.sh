#!/usr/bin/env bash

set -euo pipefail

for required_command in git git-cliff; do
	if ! command -v "${required_command}" >/dev/null 2>&1; then
		echo "required command is not installed: ${required_command}" >&2
		exit 1
	fi
done

bump="${1:-}"
output_format="${2:-version}"
case "${output_format}" in
version | --metadata) ;;
*)
	echo "usage: $0 <patch|minor|major> [--metadata]" >&2
	exit 2
	;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

candidate_files=(
	Cargo.toml
	Cargo.lock
	crates/cli/CHANGELOG.md
)

version="$(bash "${script_dir}/version.sh" bump "${bump}")"
tag="v${version}"

git-cliff \
	--config cliff.toml \
	--unreleased \
	--tag "${tag}" \
	--prepend crates/cli/CHANGELOG.md

bash "${script_dir}/check.sh" HEAD >/dev/null

case "${output_format}" in
version)
	printf '%s\n' "${version}"
	;;
--metadata)
	printf '%s\n' "${version}" "${candidate_files[@]}"
	;;
esac
