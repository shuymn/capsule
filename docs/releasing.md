# Release workflow

Use this workflow to prepare, approve, promote, and distribute a release.

## Contract

- Declare the product version once in `[workspace.package].version`. Keep every workspace crate private, use `version.workspace = true`, keep internal dependencies path-only, and synchronize local package versions in `Cargo.lock`.
- Use the merged Release PR's head commit as `candidate_sha`. Merge with a merge commit so that reviewed head remains reachable from `main`.
- Create `v{version}` at the validated candidate SHA. An existing tag at that SHA is a successful retry; a tag at another SHA is a conflict and must remain unchanged.
- Publish the GitHub Release only after all required archives, checksums, and attestations succeed. Keep published releases immutable; repair drafts or issue a corrective version.
- Use the Release PR, candidate commit, version tag, and GitHub Release as release state.

## Prepare and promote

1. Dispatch [Release PR](../.github/workflows/release-pr.yml) with `patch`, `minor`, or `major`.
2. Review the generated workspace version, `Cargo.lock`, and `crates/cli/CHANGELOG.md`; wait for pull-request CI to pass.
3. Merge the PR with a merge commit, then wait for merged `main` CI to pass.
4. Dispatch [Release Promote](../.github/workflows/release-promote.yml) with that merged PR number. Confirm the derived tag and candidate SHA.
5. Confirm [Release](../.github/workflows/release.yml) publishes the required assets. Nix cache publication and Maltmill's Homebrew update complete independently.

Candidate generation uses [candidate.sh](../scripts/release/candidate.sh). [propose.sh](../scripts/release/propose.sh) verifies the GitHub-signed candidate commit before updating `release/vX.Y.Z` and compares the previous branch SHA to prevent overwriting concurrent updates. Candidate generation creates neither tags nor GitHub Releases.

Promotion requires a merged PR targeting `main`, checks out its head, verifies reachability from `main`, and runs `task release:check` before creating write credentials. Use GitHub App credentials for candidate-branch updates and tag pushes so downstream workflows run.

## Local validation

- Run `task release:check` at the candidate checkout. It validates version inheritance, resolved package versions, `Cargo.lock`, changelog sections, and existing-tag identity, then prints the tag without repository writes. Set `RELEASE_SHA` to require an exact `HEAD`.
- Run `task release:test` when changing release validation; it exercises valid, idempotent, and rejected states in temporary repositories.
- To prepare locally, run `task release:prepare BUMP=patch` with the selected increment and the git-cliff version pinned in `Release PR`. This updates release files; review the resulting diff.

## Distribution

Preserve these Maltmill-compatible archive names and their `.sha256` files:

- `capsule-vX.Y.Z-darwin-arm64.tar.gz`
- `capsule-vX.Y.Z-linux-amd64.tar.gz`
- `capsule-vX.Y.Z-linux-arm64.tar.gz`

Each archive contains `capsule`. Keep artifact attestation in the build gate and checksum verification in the publication gate. Validate these contracts and Homebrew consumption with a prerelease before replacing the distribution workflow.

[Release Nix Cache](../.github/workflows/release-nix.yml) consumes version tags independently. Its failure does not block or roll back the GitHub Release. Maltmill consumes published GitHub Releases asynchronously.

## Recovery

| State | Action |
|---|---|
| Candidate generation failed or needs regeneration before merge | Re-dispatch `Release PR` with the intended increment; review the updated candidate. |
| Promotion stopped before tag creation | Re-dispatch `Release Promote` with the same PR number. |
| Tag already points to the candidate | Treat promotion as complete. |
| Tag points to another commit | Investigate without moving it; prepare a corrective version. |
| Binary release failed; release is absent or draft | Re-run the failed `Release` workflow. |
| Published release lacks required assets | Prepare a corrective release; preserve the published release. |
| Nix cache publication failed | Re-run `Release Nix Cache` independently. |
