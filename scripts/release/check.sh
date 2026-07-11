#!/usr/bin/env bash

set -euo pipefail

for required_command in cargo git grep jq; do
	if ! command -v "${required_command}" >/dev/null 2>&1; then
		echo "required command is not installed: ${required_command}" >&2
		exit 1
	fi
done

release_ref="${1:-HEAD}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

if ! release_sha="$(git rev-parse --verify "${release_ref}^{commit}" 2>/dev/null)"; then
	echo "release ref does not resolve to a commit: ${release_ref}" >&2
	exit 1
fi

head_sha="$(git rev-parse --verify HEAD)"
if [[ "${head_sha}" != "${release_sha}" ]]; then
	echo "checked out ${head_sha}, expected release commit ${release_sha}" >&2
	exit 1
fi

version="$(bash "${script_dir}/version.sh" check)"
metadata="$(cargo metadata --format-version 1 --no-deps --locked)"

while IFS=$'\t' read -r package_name package_version; do
	if ! awk -v expected_name="${package_name}" -v expected_version="${package_version}" '
    /^\[\[package\]\]$/ {
      if (name == expected_name && version == expected_version && source == "") {
        found = 1
      }
      name = ""
      version = ""
      source = ""
      next
    }
    /^name = "/ {
      name = $0
      sub(/^name = "/, "", name)
      sub(/"$/, "", name)
      next
    }
    /^version = "/ {
      version = $0
      sub(/^version = "/, "", version)
      sub(/"$/, "", version)
      next
    }
    /^source = "/ {
      source = $0
      sub(/^source = "/, "", source)
      sub(/"$/, "", source)
    }
    END {
      if (name == expected_name && version == expected_version && source == "") {
        found = 1
      }
      exit(found ? 0 : 1)
    }
  ' Cargo.lock; then
		echo "Cargo.lock does not contain ${package_name} ${package_version}" >&2
		exit 1
	fi
done < <(jq -r '.packages[] | [.name, .version] | @tsv' <<<"${metadata}")

tag="v${version}"
if ! git check-ref-format "refs/tags/${tag}" >/dev/null 2>&1; then
	echo "workspace version does not produce a valid tag: ${tag}" >&2
	exit 1
fi

changelog="crates/cli/CHANGELOG.md"
if [[ ! -f "${changelog}" ]]; then
	echo "changelog does not exist: ${changelog}" >&2
	exit 1
fi
if ! grep -Fq '## [Unreleased]' "${changelog}"; then
	echo "missing changelog Unreleased section" >&2
	exit 1
fi
if ! grep -Fq "## [${version}]" "${changelog}"; then
	echo "missing changelog entry for ${version}" >&2
	exit 1
fi

if git show-ref --verify --quiet "refs/tags/${tag}"; then
	tagged_sha="$(git rev-parse --verify "${tag}^{commit}")"
	if [[ "${tagged_sha}" != "${release_sha}" ]]; then
		echo "tag ${tag} points to ${tagged_sha}, expected ${release_sha}" >&2
		exit 1
	fi
fi

printf '%s\n' "${tag}"
