#!/usr/bin/env bash

set -euo pipefail

for required_command in gh git jq; do
	if ! command -v "${required_command}" >/dev/null 2>&1; then
		echo "required command is not installed: ${required_command}" >&2
		exit 1
	fi
done

: "${BUMP:?BUMP is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_RUN_ATTEMPT:?GITHUB_RUN_ATTEMPT is required}"
: "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
: "${GITHUB_STEP_SUMMARY:?GITHUB_STEP_SUMMARY is required}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git rev-parse --show-toplevel)"
cd "${repo_root}"

base_sha="$(git rev-parse HEAD)"
candidate_metadata="$(bash "${script_dir}/candidate.sh" "${BUMP}" --metadata)"
version="${candidate_metadata%%$'\n'*}"
candidate_file_list="${candidate_metadata#*$'\n'}"
candidate_files=()
while IFS= read -r candidate_file; do
	[[ -n "${candidate_file}" ]] && candidate_files+=("${candidate_file}")
done <<<"${candidate_file_list}"
if ((${#candidate_files[@]} == 0)); then
	echo "candidate generator did not report any files" >&2
	exit 1
fi

release_branch="release/v${version}"
release_ref="refs/heads/${release_branch}"
temporary_branch="release-candidate/${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
temporary_ref="refs/heads/${temporary_branch}"
title="chore(release): prepare v${version}"
body="$(printf '%s\n\n%s\n%s' \
	"Automated release candidate for v${version}." \
	"- SemVer increment: \`${BUMP}\`" \
	"- Base commit: \`${base_sha}\`")"

repository_owner="${GITHUB_REPOSITORY%%/*}"
repository_name="${GITHUB_REPOSITORY#*/}"
# GraphQL expands its own variables; the shell must not.
# shellcheck disable=SC2016
repository_state="$(gh api graphql \
	-f query='query($owner: String!, $name: String!, $qualifiedName: String!) { repository(owner: $owner, name: $name) { id ref(qualifiedName: $qualifiedName) { target { oid } } } }' \
	-f owner="${repository_owner}" \
	-f name="${repository_name}" \
	-f qualifiedName="${release_ref}")"
repository_id="$(jq -r '.data.repository.id' <<<"${repository_state}")"
previous_sha="$(jq -r '.data.repository.ref.target.oid // empty' <<<"${repository_state}")"

# shellcheck disable=SC2016
temporary_ref_id="$(gh api graphql \
	-f query='mutation($repositoryId: ID!, $name: String!, $oid: GitObjectID!) { createRef(input: { repositoryId: $repositoryId, name: $name, oid: $oid }) { ref { id } } }' \
	-f repositoryId="${repository_id}" \
	-f name="${temporary_ref}" \
	-f oid="${base_sha}" \
	--jq '.data.createRef.ref.id')"
cleanup_temporary_ref() {
	if [[ -n "${temporary_ref_id:-}" ]]; then
		# shellcheck disable=SC2016
		gh api graphql \
			-f query='mutation($refId: ID!) { deleteRef(input: { refId: $refId }) { clientMutationId } }' \
			-f refId="${temporary_ref_id}" \
			>/dev/null 2>&1 || true
	fi
}
trap cleanup_temporary_ref EXIT

additions='[]'
for candidate_file in "${candidate_files[@]}"; do
	contents="$(jq -Rrs '@base64' <"${candidate_file}")"
	additions="$(jq \
		--arg path "${candidate_file}" \
		--arg contents "${contents}" \
		'. + [{ path: $path, contents: $contents }]' \
		<<<"${additions}")"
done

# shellcheck disable=SC2016
commit_query='mutation($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid signature { isValid state wasSignedByGitHub } } } }'
candidate_result="$(jq -n \
	--arg query "${commit_query}" \
	--arg repository "${GITHUB_REPOSITORY}" \
	--arg branch "${temporary_branch}" \
	--arg expectedHeadOid "${base_sha}" \
	--arg headline "${title}" \
	--argjson additions "${additions}" \
	'{
      query: $query,
      variables: {
        input: {
          branch: {
            repositoryNameWithOwner: $repository,
            branchName: $branch
          },
          expectedHeadOid: $expectedHeadOid,
          message: { headline: $headline },
          fileChanges: { additions: $additions }
        }
      }
    }' | gh api graphql --input -)"
candidate_sha="$(jq -r '.data.createCommitOnBranch.commit.oid' <<<"${candidate_result}")"
if ! jq -e '
  .data.createCommitOnBranch.commit.signature
  | .isValid == true and .state == "VALID" and .wasSignedByGitHub == true
' <<<"${candidate_result}" >/dev/null; then
	signature_state="$(jq -r \
		'.data.createCommitOnBranch.commit.signature.state // "UNSIGNED"' \
		<<<"${candidate_result}")"
	echo "candidate commit is not verified by GitHub: ${candidate_sha} (${signature_state})" >&2
	exit 1
fi

zero_sha="$(printf '0%.0s' {1..40})"
if [[ -n "${previous_sha}" ]]; then
	release_before_sha="${previous_sha}"
	force_release_update=true
else
	release_before_sha="${zero_sha}"
	force_release_update=false
fi
ref_updates="$(jq -n \
	--arg releaseRef "${release_ref}" \
	--arg releaseBeforeSha "${release_before_sha}" \
	--arg candidateSha "${candidate_sha}" \
	--arg temporaryRef "${temporary_ref}" \
	--arg zeroSha "${zero_sha}" \
	--argjson forceReleaseUpdate "${force_release_update}" \
	'[
      {
        name: $releaseRef,
        beforeOid: $releaseBeforeSha,
        afterOid: $candidateSha,
        force: $forceReleaseUpdate
      },
      {
        name: $temporaryRef,
        beforeOid: $candidateSha,
        afterOid: $zeroSha,
        force: false
      }
    ]')"
# shellcheck disable=SC2016
update_refs_query='mutation($repositoryId: ID!, $refUpdates: [RefUpdate!]!) { updateRefs(input: { repositoryId: $repositoryId, refUpdates: $refUpdates }) { clientMutationId } }'
jq -n \
	--arg query "${update_refs_query}" \
	--arg repositoryId "${repository_id}" \
	--argjson refUpdates "${ref_updates}" \
	'{
      query: $query,
      variables: {
        repositoryId: $repositoryId,
        refUpdates: $refUpdates
      }
    }' | gh api graphql --input - >/dev/null
temporary_ref_id=""

pr_number="$(gh pr list \
	--repo "${GITHUB_REPOSITORY}" \
	--base main \
	--head "${release_branch}" \
	--state open \
	--json number \
	--jq '.[0].number // empty')"
if [[ -n "${pr_number}" ]]; then
	gh pr edit "${pr_number}" \
		--repo "${GITHUB_REPOSITORY}" \
		--title "${title}" \
		--body "${body}"
	pr_url="$(gh pr view "${pr_number}" --repo "${GITHUB_REPOSITORY}" --json url --jq .url)"
else
	pr_url="$(gh pr create \
		--repo "${GITHUB_REPOSITORY}" \
		--base main \
		--head "${release_branch}" \
		--title "${title}" \
		--body "${body}")"
fi

echo "Release PR: ${pr_url}" >>"${GITHUB_STEP_SUMMARY}"
