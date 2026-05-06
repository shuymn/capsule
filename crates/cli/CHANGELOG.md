# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3](https://github.com/shuymn/capsule/compare/v0.1.2...v0.1.3) - 2026-05-06

### Other

- update Cargo.toml dependencies

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
