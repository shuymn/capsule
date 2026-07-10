# Release workflow

Use this workflow to prepare, approve, promote, and distribute a capsule release. Keep candidate production replaceable; treat the validated candidate commit and version tag as the stable contract.

## Release contract

- Use `[workspace.package].version` in `Cargo.toml` as the only product version source.
- Treat `feat` commits as minor increments even before 1.0; keep `features_always_increment_minor = true` in the candidate-producer configuration.
- Use the merged Release PR's head commit as `candidate_sha`. Tag the reviewed candidate tree, not the latest `main` commit or the merge commit.
- Merge Release PRs with a merge commit. Promotion rejects a candidate that is not reachable from `main`; squash or rebase merging makes the original reviewed head unreachable.
- Derive the release tag as `v{version}`.
- Never move an existing release tag. Treat an existing tag at the same candidate as a successful retry and a tag at another commit as an incident.
- Trigger binary distribution and the Nix cache subscriber from the version tag.
- Publish the GitHub Release only after every required archive, checksum, and attestation is ready.
- Keep the published GitHub Release immutable. Repair a failed draft or issue a corrective version instead of mutating a published release.

The durable release state is the Release PR, its candidate commit, the version tag, and the GitHub Release. Do not create a separate release-state file, database, or long-lived control branch.

## Intentionally not in core

| Capability | Owner | Extension path |
|---|---|---|
| SemVer decision and changelog wording | Candidate producer and reviewer | Replace the manually dispatched release-plz workflow without changing promotion |
| Release cadence | Operator | Change the trigger that invokes the candidate producer |
| Approval policy | Repository review rules | Keep approval outside release scripts |
| Archive builder and GitHub publisher | Tag consumer | Replace `.github/workflows/release.yml` after verifying its artifact contract |
| Homebrew formula update | Maltmill in `homebrew-tap` | Consume the immutable GitHub Release asynchronously |
| Nix cache publication | Tag subscriber | Retry `.github/workflows/release-nix.yml` independently |

Do not invoke `release-plz release`. The release-plz adapter may only create or update a Release PR.

## Prepare and approve a candidate

1. Dispatch the `Release PR` workflow.
2. Review the generated version, workspace dependency versions, `Cargo.lock`, and `crates/cli/CHANGELOG.md` changes.
3. Let the normal pull-request CI complete.
4. Check out the Release PR head and run `task release:check` if the generated release files need local verification.
5. Merge the Release PR with a merge commit. Do not squash or rebase it.
6. Wait for the merged `main` CI run to succeed.

The release-plz adapter currently uses a previous-tag manifest as its comparison baseline. Keep that workaround confined to `.github/workflows/release-pr.yml`; remove it when [release-plz issue #2595](https://github.com/release-plz/release-plz/issues/2595) is resolved. `task package:check` remains a CI gate because this adapter requires a registry-compatible workspace package graph even though release policy disables crates.io publishing. Do not add `publish = false` to the `capsule-cli` manifest while using release-plz 0.3.159; it excludes that package from candidate updates. Keep publication disabled in `release-plz.toml` instead.

## Promote the candidate

1. Dispatch the `Release Promote` workflow with the merged Release PR number.
2. Confirm that the workflow derives the expected `vX.Y.Z` tag.
3. Confirm that the `Release` workflow publishes all three binary archives and checksums.
4. Treat `Release Nix Cache` as an independent best-effort subscriber.
5. Let Maltmill update the Homebrew formula asynchronously.

Promotion resolves the candidate SHA from the merged Release PR, then validates the exact checkout, reachability from `main`, the shared workspace version, `Cargo.lock`, the changelog entry, and any existing tag before it creates write credentials. The GitHub App pushes the tag so the tag-triggered workflows run.

## Retry and recovery

- Re-dispatch `Release PR` to update the existing candidate PR.
- Re-dispatch `Release Promote` with the same Release PR number when promotion stopped before tag creation.
- Treat promotion as complete when the tag already resolves to the candidate SHA.
- Do not force-update a conflicting tag. Investigate it and prepare a corrective version.
- Re-run a failed binary release while its GitHub Release is absent or draft.
- Do not add missing assets to an already-published immutable release; prepare a corrective release.
- Re-run the Nix cache workflow independently. Its failure does not roll back or block the GitHub Release.

## Automation seam

Future automation may invoke `Release PR` after a `main` push or on a schedule. After that PR is merged, a controller may call `Release Promote` through `workflow_call` with the Release PR number. Promotion must continue to derive the candidate SHA from that merged PR. Do not change `task release:check`, the tag format, or the tag-triggered consumers when adding those triggers.

Do not select `latest main` during promotion. The candidate SHA is the approved release identity even when `main` advances before promotion runs.

## Acceptance criteria

- WHEN the Release PR workflow is dispatched, the system SHALL update a Release PR without creating a tag or GitHub Release.
- WHEN promotion is requested, the system SHALL require a merged Release PR targeting `main` and derive its candidate SHA from that PR.
- IF the candidate is not reachable from `main`, the system SHALL fail before creating write credentials or tags.
- IF workspace versions, `Cargo.lock`, or the changelog disagree, the system SHALL fail before creating write credentials or tags.
- IF the release tag does not exist, the system SHALL create it at the validated candidate SHA.
- IF the release tag resolves to the candidate SHA, the system SHALL complete successfully without mutation.
- IF the release tag resolves to another commit, the system SHALL fail without moving the tag.
- WHILE any required binary artifact is missing, the system SHALL not publish the GitHub Release.
- WHEN a downstream subscriber fails, the system SHALL keep the published release and allow that subscriber to retry independently.

## Distribution adapter constraints

Keep the current release workflow until its artifact consumers can accept Rust target-triple names. `dist` currently emits names such as `capsule-cli-aarch64-apple-darwin.tar.gz`, while [Maltmill v1.5.0](https://github.com/Songmu/maltmill/blob/v1.5.0/cmd_new.go#L130-L178) recognizes the existing `darwin-arm64`, `linux-amd64`, and `linux-arm64` suffixes. `dist` does not currently provide archive renaming that preserves those names ([#1371](https://github.com/axodotdev/cargo-dist/issues/1371), [#2428](https://github.com/axodotdev/cargo-dist/issues/2428)).

Revisit `dist` after Maltmill supports target triples or Homebrew publication moves to another adapter. Verify archive names, three target builds, checksums, attestations, immutable-release retry behavior, and Homebrew updates with a prerelease before replacing `.github/workflows/release.yml`.
