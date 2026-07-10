# ADR: Release candidate promotion contract

## Status

Accepted

## Context

The previous `Tag` workflow ran both `release-plz release` and `release-plz release-pr` after `main` CI. It treated the latest `main` SHA as a release gate and duplicated authentication, checkout, and release-tool setup across two jobs.

The repository does not publish crates to crates.io, but release-plz still requires a registry-compatible package graph to calculate and validate a Release PR. Historical tags predate the workspace dependency version requirements, so candidate production also requires a previous-tag manifest workaround.

Binary distribution has different requirements: three platform archives, checksums, attestations, immutable GitHub Releases, Maltmill-compatible asset names, and a best-effort Nix cache. These requirements do not belong to candidate production or promotion.

## Decision

Split the release flow into named stages with a tag boundary.

1. `Release PR` is a manually dispatched candidate-producer adapter. It may create or update a Release PR but cannot create tags or GitHub Releases.
2. The Release PR head commit is the candidate identity. Merge it with a merge commit so the reviewed head remains reachable from `main`.
3. `task release:check` validates the checked-out candidate without network writes. It derives `v{workspace version}` and verifies workspace versions, `Cargo.lock`, the changelog, and existing-tag identity.
4. `Release Promote` accepts a Release PR number, requires that PR to be merged into `main`, and derives its candidate SHA from the PR head. It verifies reachability from `main`, runs the release contract check, and creates an annotated tag only after all read-only gates pass.
5. The existing `Release` workflow remains the binary builder and immutable GitHub Release publisher behind the version-tag boundary.
6. `Release Nix Cache` is an independent, best-effort version-tag subscriber.
7. Maltmill remains an asynchronous subscriber to published GitHub Releases.

Use a GitHub App installation token for candidate branch pushes and release tag pushes. A tag pushed with the repository `GITHUB_TOKEN` would not trigger the downstream tag workflows.

## Rejected alternatives

### Use release-plz as the end-to-end release controller

This couples candidate calculation, tag selection, and promotion to release-plz's package and registry model. It also makes its historical-manifest workaround part of the release core.

### Tag the latest `main` commit

This can skip a reviewed release candidate when `main` advances before an earlier CI run completes. It also changes the artifact tree after review.

### Tag the Release PR merge commit

The merge commit may include base-branch changes that were not part of the reviewed candidate head. Existing capsule releases tag the Release PR head, so retaining that identity is both stricter and compatible with release history.

### Replace binary distribution with `dist` immediately

The generated archive names use Rust target triples. Maltmill v1.5.0 recognizes the current `arm64` and `amd64` asset names, and `dist` does not currently support renaming archives to that contract. Forking the generated workflow to add aliases would recreate the workflow-local complexity this decision removes.

## Consequences

- Manual and future automated promotion use the same candidate-SHA contract.
- Advancing `main` no longer causes a validated candidate to be skipped.
- Candidate-producer failures cannot create or move release tags.
- Existing tag/SHA pairs are idempotent; conflicting tags fail closed.
- Release PRs must continue to use merge commits. A repository-wide switch to squash or rebase merging requires revisiting candidate identity.
- The release-plz registry workaround and `task package:check` remain adapter costs, not product release invariants.
- Nix cache failures are isolated from binary publication.

## Revisit triggers

- release-plz can compare the historical git release without a registry-compatible manifest, or the candidate producer is replaced.
- Release PRs can no longer use merge commits.
- Maltmill supports Rust target-triple asset names, `dist` supports archive renaming, or Homebrew publication moves to another adapter.
- The required artifact set or immutable-release policy changes.
