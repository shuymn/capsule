# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/shuymn/capsule/compare/v0.3.0...v0.4.0) - 2026-07-11

### Added

- *(daemon)* support CAPSULE_SOCKET_PATH env var
- *(connect)* gracefully handle Nix reconnect
- *(nix)* add Cachix binary cache support
- *(ci)* add Nix workflow
- add Nix flake with declarative daemon
- centralize workspace path dependencies
- add package manifest validation
- *(tag)* gate release on SHA staleness check
- *(release)* handle publish-from-draft safely
- secure release pipeline with verification

### Fixed

- *(daemon)* handle Nix-managed startup
- *(nix)* validate module socket paths
- *(connect)* propagate reconnect_daemon errors
- *(git)* clear git-local env vars in subprocess
- *(tag)* harden checkout with SHA verification
- *(protocol)* satisfy new Clippy lints

### Other

- *(release)* extract candidate proposal script
- *(release)* create verified candidate commits
- *(release)* inline app identity lookup
- *(release)* avoid template injection
- *(release)* use GitHub App commit identity
- *(release)* set up Rust explicitly
- *(release)* avoid duplicate branch lookup
- *(release)* verify bumped lock entries
- *(release)* own candidate generation
- *(release)* minor bump on feat before 1.0
- *(deps)* update taiki-e/install-action action to v2.82.10 (#160)
- *(deps)* update taiki-e/install-action action to v2.82.10
- *(deps)* update cargo-binstall to v1.20.1 (#159)
- *(deps)* update cargo-binstall to v1.20.1
- *(release)* declare workflow permissions
- *(release)* split candidate promotion flow
- *(deps)* update taiki-e/install-action action to v2.82.9 (#157)
- *(deps)* update taiki-e/install-action action to v2.82.9
- *(deps)* update cachix/install-nix-action action to v31.10.6 (#156)
- *(deps)* update cachix/install-nix-action action to v31.10.6
- *(daemon)* cover launchd Nix marker
- *(release)* keep cache push nonblocking
- *(release)* make nix-cache nonblocking
- *(release)* remove nix-cache from needs
- *(systemd)* extract testable service paths
- require stateVersion in Nix module snippets
- *(nix)* cancel in-progress CI runs on new pushes
- *(nix)* run tests for all workspace members
- *(deps)* update taiki-e/install-action action to v2.82.8 (#155)
- *(deps)* update taiki-e/install-action action to v2.82.8
- *(tooling)* update CI cache documentation
- normalize workspace dependency syntax
- *(tag)* trigger on CI completion instead of push
- *(tag)* switch trigger from workflow_run to push
- *(release)* add git_only config
- add --locked to cargo package check
- *(release)* remove unused git_only config
- *(deps)* update rust crate time to v0.3.53
- *(deps)* update taiki-e/install-action action to v2.82.7 (#151)
- *(deps)* update taiki-e/install-action action to v2.82.7
- *(deps)* update taiki-e/install-action action to v2.82.6 (#149)
- *(deps)* update taiki-e/install-action action to v2.82.6
- *(deps)* update rust crate time to v0.3.51
- *(deps)* update actions/checkout action to v7
- *(deps)* update release-plz/action action to v0.5.130
- *(deps)* update rust crate anyhow to v1.0.103 (#147)
- *(deps)* update rust crate anyhow to v1.0.103
- *(deps)* update docker/dockerfile docker tag to v1.25 (#146)
- *(deps)* update docker/dockerfile docker tag to v1.25
- *(deps)* update taiki-e/install-action action to v2.82.0 (#145)
- *(deps)* update taiki-e/install-action action to v2.82.5
- *(deps)* update actions-rust-lang/setup-rust-toolchain action to v1.17.0 (#148)
- *(deps)* update actions-rust-lang/setup-rust-toolchain action to v1.17.0
- *(deps)* update taiki-e/install-action action to v2.81.8 (#140)
- *(deps)* update taiki-e/install-action action to v2.81.8
- *(deps)* update taiki-e/install-action action to v2.81.7 (#139)
- *(deps)* update taiki-e/install-action action to v2.81.7
- *(deps)* update taiki-e/install-action action to v2.81.2 (#138)
- *(deps)* update taiki-e/install-action action to v2.81.2
- *(deps)* update actions/checkout action to v6.0.3 (#137)
- *(deps)* update actions/checkout action to v6.0.3
- *(deps)* update taiki-e/install-action action to v2.81.1 (#136)
- *(deps)* update taiki-e/install-action action to v2.81.1

## [0.3.0](https://github.com/shuymn/capsule/compare/v0.2.0...v0.3.0) - 2026-06-08

### Added

- add slot config for custom module placement

## [0.2.0](https://github.com/shuymn/capsule/compare/v0.1.2...v0.2.0) - 2026-06-06

### Fixed

- *(init)* change unescape_field to assign
- *(cli)* validate pid before signal_process

### Other

- remove version field from all messages

## [0.1.2](https://github.com/shuymn/capsule/compare/v0.1.1...v0.1.2) - 2026-03-23

### Added

- *(daemon)* update service definition on restart

### Other

- *(daemon)* streamline action matching for install and uninstall commands
- *(daemon)* simplify action handling for install and uninstall commands

## [0.1.1](https://github.com/shuymn/capsule/compare/v0.1.0...v0.1.1) - 2026-03-23

### Other

- update Cargo.toml dependencies

## [0.1.0](https://github.com/shuymn/capsule/compare/v0.0.1...v0.1.0) - 2026-03-23

### Added

- *(cli)* add --version flag
- support systemd socket activation on Linux
- *(daemon)* forward env vars to launchd plist
- support vim mode character indicator
- *(cli)* add preset subcommand
- *(git)* show short OID for detached HEAD
- *(prompt-bench)* add cached benchmark phase
- replace prompt bench with Rust crate
- *(daemon)* enhance socket management in install process
- introduce typed generations throughout IPC
- *(daemon)* expose metrics via status RPC
- *(daemon)* hot-reload config on mtime
- *(connect)* protocol translation layer
- *(cli)* support env_var_names from daemon HelloAck
- *(cli)* add ServiceManager for daemon install/uninstall
- *(cli)* daemon restart on binary update
- *(protocol)* add env_vars to wire format
- *(core)* config file with module customization
- *(cli)* daemon install/uninstall for launchd
- *(cli)* reconnect relay when daemon restarts
- *(core)* add Starship-style toolchain version
- *(core)* add Starship-style prompt rendering
- *(cli)* add build ID handshake for stale daemon
- *(cli)* add flock to prevent dual daemon startup
- *(cli)* implement Theme 7 E2E integration
- *(cli)* add daemon, connect, init subcommands

### Fixed

- *(daemon)* require HelloAck for readiness
- *(connect)* keep daemon alive when spawning
- *(init)* harden init.zsh, add function tests
- *(cli)* shutdown runtime to prevent connect hang

### Other

- shorten test function names
- drop CAPSULE_SOCK_DIR socket override
- *(connect)* add default request timeout
- reduce test duplication across modules
- update source resolution description
- add Japanese README
- add demo GIF to README
- add dockerized linux checks
- update for Linux support
- rewrite README
- replace color fields with StyleConfig
- *(daemon)* replace TTL cache with LRU
- *(cli)* factor daemon into submodules
- drop redundant default snapshot tests
- *(readme)* document config-driven prompt
- *(cli)* extract socket wait helper for e2e
- *(init)* remove protocol logic from shell
- *(cli)* refactor e2e tests into shared test_support
- update README and architecture for launchd
- add bench:prompt task with README docs
- *(cli)* add TMPDIR environment variable to init test
- *(cli)* add E2E connect relay test
- add protocol/core/cli workspace
- initialize from template
- Initial commit
