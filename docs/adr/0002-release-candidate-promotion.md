---
status: accepted
---

# Release candidate promotion

Use the merged Release PR's reviewed head as the release identity and promote it to an immutable `v{workspace version}` tag after contract validation. Require a merge commit so that head remains reachable from `main` and later changes to `main` cannot alter the reviewed release tree. Keep binary publication and Nix/Homebrew subscribers behind the immutable tag and release boundaries defined in the [release workflow](../releasing.md) so they can retry independently.
