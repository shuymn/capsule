#!/usr/bin/env bash

set -euo pipefail

for required_command in cargo git grep jq; do
	if ! command -v "${required_command}" >/dev/null 2>&1; then
		echo "required command is not installed: ${required_command}" >&2
		exit 1
	fi
done

repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

workspace_version() {
	local metadata version version_count manifest_path package_name public_packages

	metadata="$(cargo metadata --format-version 1 --no-deps --locked)"
	version_count="$(jq '[.packages[].version] | unique | length' <<<"${metadata}")"
	if [[ "${version_count}" != "1" ]]; then
		echo "workspace packages do not share one version" >&2
		jq -r '.packages[] | "  \(.name): \(.version)"' <<<"${metadata}" >&2
		return 1
	fi

	while IFS=$'\t' read -r package_name manifest_path; do
		if ! grep -Eq '^[[:space:]]*version\.workspace[[:space:]]*=[[:space:]]*true[[:space:]]*$' "${manifest_path}"; then
			echo "workspace package ${package_name} does not inherit workspace.package.version" >&2
			return 1
		fi
	done < <(jq -r '.packages[] | [.name, .manifest_path] | @tsv' <<<"${metadata}")

	public_packages="$(jq -r '.packages[] | select(.publish != []) | .name' <<<"${metadata}")"
	if [[ -n "${public_packages}" ]]; then
		while IFS= read -r package_name; do
			echo "workspace package ${package_name} is not private" >&2
		done <<<"${public_packages}"
		return 1
	fi

	version="$(jq -r '.packages[0].version // empty' <<<"${metadata}")"
	if [[ -z "${version}" ]]; then
		echo "cargo metadata does not contain workspace packages" >&2
		return 1
	fi

	printf '%s\n' "${version}"
}

next_version() {
	local version="$1"
	local bump="$2"

	if [[ ! "${version}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
		echo "workspace version is not stable SemVer: ${version}" >&2
		return 1
	fi

	local major="${BASH_REMATCH[1]}"
	local minor="${BASH_REMATCH[2]}"
	local patch="${BASH_REMATCH[3]}"

	case "${bump}" in
	patch)
		printf '%d.%d.%d\n' "${major}" "${minor}" "$((patch + 1))"
		;;
	minor)
		printf '%d.%d.0\n' "${major}" "$((minor + 1))"
		;;
	major)
		printf '%d.0.0\n' "$((major + 1))"
		;;
	*)
		echo "unsupported release bump: ${bump}; expected patch, minor, or major" >&2
		return 1
		;;
	esac
}

update_manifest() {
	local version="$1"
	local tmp_manifest

	if ! command -v awk >/dev/null 2>&1; then
		echo "required command is not installed: awk" >&2
		return 1
	fi

	tmp_manifest="$(mktemp "${TMPDIR:-/tmp}/capsule-Cargo.toml.XXXXXX")"
	if ! awk -v next_version="${version}" '
    /^\[/ {
      section = $0
    }
    section == "[workspace.package]" && /^version = "/ {
      print "version = \"" next_version "\""
      workspace_version_count++
      next
    }
    {
      print
    }
    END {
      if (workspace_version_count != 1) {
        print "expected exactly one workspace package version" > "/dev/stderr"
        exit 1
      }
    }
  ' Cargo.toml >"${tmp_manifest}"; then
		rm -f "${tmp_manifest}"
		return 1
	fi

	cp "${tmp_manifest}" Cargo.toml
	rm -f "${tmp_manifest}"
}

command="${1:-check}"
case "${command}" in
check)
	workspace_version
	;;
bump)
	bump="${2:-}"
	current_version="$(workspace_version)"
	target_version="$(next_version "${current_version}" "${bump}")"

	if ! git diff --quiet -- Cargo.toml Cargo.lock ||
		! git diff --cached --quiet -- Cargo.toml Cargo.lock; then
		echo "Cargo.toml or Cargo.lock contains uncommitted changes" >&2
		exit 1
	fi

	update_manifest "${target_version}"
	cargo update --workspace

	actual_version="$(workspace_version)"
	if [[ "${actual_version}" != "${target_version}" ]]; then
		echo "prepared ${actual_version}, expected ${target_version}" >&2
		exit 1
	fi
	printf '%s\n' "${target_version}"
	;;
*)
	echo "unsupported version command: ${command}; expected check or bump" >&2
	exit 1
	;;
esac
