# capsule

English・[日本語](README.ja.md)

A zsh prompt that never makes you wait to type. `capsule` is written in Rust for macOS and Linux: each shell owns one background worker that gathers Git and tool information while zsh keeps accepting input.

<p align="center">
  <img src="assets/vhs/readme-prompt.gif" alt="capsule prompt demo">
</p>

## Why capsule

- **Input is never blocked.** Prompts that compute everything in `precmd` make you wait for `git status` after every Enter. Capsule draws the prompt immediately, keeps the last confirmed information while the directory and environment are unchanged, and swaps in fresh results when they arrive.
- **No module zoo.** There are no built-in modules for specific tools. Node, AWS, Kubernetes, and anything else are a few lines of configuration that read an environment variable, a file, or a command's output.
- **zsh only, on purpose.** Supporting a single shell lets the integration rely on zsh's hooks and line editor instead of a lowest common denominator.
- **Prompt data stays data.** Branch names, file contents, and command output are displayed literally. They are never expanded as prompt escapes or shell code ([threat model](docs/threat-model.md)).

## Measured

Pressing Enter on an empty line in a large repository (50,000 tracked files, 200 modified, 200 untracked) right after the branch was switched from another terminal. Both tools use their default configuration. Values are p50 / p90 of 50 interleaved samples per tool on GitHub-hosted runners.

| Runner | Tool | Input blocked | Fresh branch shown |
|---|---|---|---|
| ubuntu-latest (AMD EPYC, 4 vCPU) | capsule 1.1.1 | 1.4 / 1.4 ms | 53 / 58 ms |
| | starship 1.26.0 | 73 / 79 ms | 73 / 79 ms |
| macos-latest (Apple M1 VM, 3 vCPU) | capsule 1.1.1 | 1.7 / 2.4 ms | 82 / 111 ms |
| | starship 1.26.0 | 126 / 247 ms | 126 / 247 ms |

*Input blocked* is the time until zsh accepts typing again; a plain zsh prompt takes 0.4 ms. *Fresh branch shown* is the time until the new branch name is on screen. The numbers compare tools on the same runner, not absolute performance. See the [full results](docs/benchmarks/2026-10-17-v1.1.1.md) and the [method](docs/benchmarking.md#user-visible-prompt-latency); anyone can repeat it with the Prompt latency workflow.

## Prompt

```text
<directory> on <git branch> [indicators] via <custom value> took <duration>
<optional custom values> at <optional time> ❯
```

The two-line prompt shows the directory, Git, custom values, command duration, and optional time. The prompt character reflects the last command's status and changes to `❮` in vi command mode.

## Installation

Requirements: macOS or Linux and zsh. Git information requires `git` on the shell's exported `PATH`.

```sh
brew install shuymn/tap/capsule
# or
mise use -g github:shuymn/capsule
```

Add this line to `.zshrc`:

```zsh
eval "$(capsule init zsh)"
```

Upgrading from 0.x (shared daemon)? Complete the [migration steps](docs/migration.md) before replacing the binary.

### Nix

```sh
nix run github:shuymn/capsule -- --version
nix profile install github:shuymn/capsule
```

The profile install provides the binary; add the `.zshrc` line yourself. Declarative modules install the binary and add that initialization automatically:

```nix
# Add inputs.capsule.url = "github:shuymn/capsule" to your flake.
{
  imports = [ inputs.capsule.homeManagerModules.default ];
  programs.capsule.enable = true;
}
```

Use `inputs.capsule.nixosModules.default` or `inputs.capsule.darwinModules.default` in the matching system configuration. Choose one integration owner. Set `programs.capsule.enableZshIntegration = false` to manage initialization yourself.

## Configuration

Capsule works without a configuration. Run `capsule preset` to print [editable examples](examples/config.toml), keep the modules you need, and save them to `~/.config/capsule/config.toml`. This module shows the Node version only inside a project:

```toml
schema_version = 2

[[module]]
name = "node"
when.files = ["package.json"]
format = "v{version}"
connector = "via"

[module.values]
version = [{ command = ["node", "--version"], regex = 'v?(\d[\d.]*)' }]
```

See the [configuration guide](docs/extending.md) for value sources, conditions, formatting, styles, and reload behavior.

## Scope

Capsule is kept small enough for one person to maintain. Tool-specific display is written as configuration rather than added as new modules, and shells other than zsh are out of scope.

## Development

Use stable Rust and Task; tests also require uv, zsh, and Git. Read [architecture](docs/architecture.md) for runtime contracts, [tooling](docs/tooling.md) for checks, [benchmarks](docs/benchmarking.md) for measurements, and [release procedures](docs/releasing.md) for publishing.
