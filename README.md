# capsule

English・[日本語](README.ja.md)

`capsule` is a Rust prompt engine for zsh on macOS and Linux. Each shell owns one worker that updates the prompt while zsh keeps accepting input.

<p align="center">
  <img src="assets/vhs/readme-prompt.gif" alt="capsule prompt demo">
</p>

## Prompt

```text
<directory> on <git branch> [indicators] via <custom value> took <duration>
<optional custom values> at <optional time> ❯
```

The two-line prompt shows the directory, Git, custom values, command duration, and optional time. The prompt character reflects the last command's status and changes to `❮` in vi command mode. Custom modules read environment variables, files, or command output.

## Installation

Requirements: macOS or Linux and zsh. Git information requires `git` on the shell's exported `PATH`.

For an existing daemon-based installation, complete the [migration steps](docs/migration.md) before replacing the binary.

```sh
brew install shuymn/tap/capsule
```

Add this line to `.zshrc`:

```zsh
eval "$(capsule init zsh)"
```

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

Capsule uses defaults without a configuration. Run `capsule preset` to print the [editable schema-v2 examples](examples/config.toml). Keep the modules you need and save the document to `~/.config/capsule/config.toml`.

See the [configuration guide](docs/extending.md) for XDG paths, value sources, conditions, formatting, styles, and reload behavior.

## Development

Use stable Rust and Task. Read [architecture](docs/architecture.md) for runtime contracts, [tooling](docs/tooling.md) for checks, and [release procedures](docs/releasing.md) for publishing.
