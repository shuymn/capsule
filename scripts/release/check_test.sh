#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
check_script="${repo_root}/scripts/release/check.sh"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

fail() {
  echo "release contract test failed: $*" >&2
  exit 1
}

write_root_manifest() {
  local fixture="$1"
  local version="$2"

  cat >"${fixture}/Cargo.toml" <<EOF
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "${version}"
edition = "2024"
EOF
}

write_member_manifest() {
  local fixture="$1"
  local member="$2"
  local name="$3"
  local version_line="$4"

  mkdir -p "${fixture}/crates/${member}/src"
  cat >"${fixture}/crates/${member}/Cargo.toml" <<EOF
[package]
name = "${name}"
${version_line}
edition.workspace = true
publish = false
EOF
  printf 'pub fn marker() {}\n' >"${fixture}/crates/${member}/src/lib.rs"
}

write_changelog() {
  local fixture="$1"
  local version="$2"

  cat >"${fixture}/crates/cli/CHANGELOG.md" <<EOF
# Changelog

## [Unreleased]

## [${version}] - 2026-07-10

- Test release.
EOF
}

commit_fixture() {
  local fixture="$1"

  git -C "${fixture}" add .
  git -C "${fixture}" commit --quiet -m "test fixture"
}

create_fixture() {
  local fixture="$1"
  local version="${2:-1.2.3}"

  mkdir -p "${fixture}"
  write_root_manifest "${fixture}" "${version}"
  write_member_manifest "${fixture}" cli capsule-cli 'version.workspace = true'
  write_member_manifest "${fixture}" helper fixture-helper 'version.workspace = true'
  write_changelog "${fixture}" "${version}"
  cargo generate-lockfile --quiet --manifest-path "${fixture}/Cargo.toml"
  git -C "${fixture}" init --quiet
  git -C "${fixture}" config user.name "Release Contract Test"
  git -C "${fixture}" config user.email "release-contract@example.invalid"
  commit_fixture "${fixture}"
}

run_success() {
  local fixture="$1"
  local expected="$2"
  local release_ref="${3:-HEAD}"
  local actual

  if ! actual="$(cd "${fixture}" && bash "${check_script}" "${release_ref}")"; then
    fail "expected success for ${fixture}"
  fi
  [[ "${actual}" == "${expected}" ]] || fail "expected ${expected}, got ${actual}"
}

run_failure() {
  local fixture="$1"
  local expected_message="$2"
  local release_ref="${3:-HEAD}"
  local output

  if output="$(cd "${fixture}" && bash "${check_script}" "${release_ref}" 2>&1)"; then
    fail "expected failure for ${fixture}, got ${output}"
  fi
  [[ "${output}" == *"${expected_message}"* ]] ||
    fail "expected error containing '${expected_message}', got '${output}'"
}

valid_fixture="${tmp_dir}/valid"
create_fixture "${valid_fixture}"
valid_sha="$(git -C "${valid_fixture}" rev-parse HEAD)"
run_success "${valid_fixture}" v1.2.3 "${valid_sha}"
git -C "${valid_fixture}" tag --annotate v1.2.3 --message "Release v1.2.3"
run_success "${valid_fixture}" v1.2.3 "${valid_sha}"

tag_mismatch_fixture="${tmp_dir}/tag-mismatch"
create_fixture "${tag_mismatch_fixture}"
git -C "${tag_mismatch_fixture}" tag --annotate v1.2.3 --message "Release v1.2.3"
printf 'changed\n' >"${tag_mismatch_fixture}/README.md"
commit_fixture "${tag_mismatch_fixture}"
run_failure "${tag_mismatch_fixture}" "tag v1.2.3 points to"

sha_mismatch_fixture="${tmp_dir}/sha-mismatch"
create_fixture "${sha_mismatch_fixture}"
old_sha="$(git -C "${sha_mismatch_fixture}" rev-parse HEAD)"
printf 'changed\n' >"${sha_mismatch_fixture}/README.md"
commit_fixture "${sha_mismatch_fixture}"
run_failure "${sha_mismatch_fixture}" "checked out" "${old_sha}"

missing_changelog_fixture="${tmp_dir}/missing-changelog"
create_fixture "${missing_changelog_fixture}"
printf '# Changelog\n\n## [Unreleased]\n' >"${missing_changelog_fixture}/crates/cli/CHANGELOG.md"
commit_fixture "${missing_changelog_fixture}"
run_failure "${missing_changelog_fixture}" "missing changelog entry for 1.2.3"

version_mismatch_fixture="${tmp_dir}/version-mismatch"
create_fixture "${version_mismatch_fixture}"
write_member_manifest "${version_mismatch_fixture}" helper fixture-helper 'version = "9.9.9"'
cargo generate-lockfile --quiet --manifest-path "${version_mismatch_fixture}/Cargo.toml"
commit_fixture "${version_mismatch_fixture}"
run_failure "${version_mismatch_fixture}" "workspace packages do not share one version"

registry_spoof_fixture="${tmp_dir}/registry-spoof"
create_fixture "${registry_spoof_fixture}"
awk '
  $0 == "name = \"capsule-cli\"" {
    target_package = 1
  }
  target_package && $0 == "version = \"1.2.3\"" {
    print
    print "source = \"registry+https://github.com/rust-lang/crates.io-index\""
    target_package = 0
    next
  }
  { print }
' "${registry_spoof_fixture}/Cargo.lock" >"${registry_spoof_fixture}/Cargo.lock.new"
mv "${registry_spoof_fixture}/Cargo.lock.new" "${registry_spoof_fixture}/Cargo.lock"
commit_fixture "${registry_spoof_fixture}"
run_failure "${registry_spoof_fixture}" "Cargo.lock does not contain capsule-cli 1.2.3"

stale_lock_fixture="${tmp_dir}/stale-lock"
create_fixture "${stale_lock_fixture}"
write_root_manifest "${stale_lock_fixture}" 1.2.4
write_changelog "${stale_lock_fixture}" 1.2.4
commit_fixture "${stale_lock_fixture}"
run_failure "${stale_lock_fixture}" "Cargo.lock does not contain capsule-cli 1.2.4"

echo "release contract tests passed"
