# Migrate to schema v2 and session workers

Use this procedure to replace a daemon-based installation with schema v2 and one worker per zsh session, or to restore the previous installation.

## Preserve the previous installation

Record the package source and version. Before replacing the old binary, use zsh to save it and the selected configuration:

```zsh
capsule_backup="$HOME/.local/state/capsule-before-v2-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$capsule_backup"
cp "$(command -v capsule)" "$capsule_backup/capsule"
capsule --version > "$capsule_backup/version"

if [[ ${(t)XDG_CONFIG_HOME} == *export* ]]; then
  capsule_config="${XDG_CONFIG_HOME:-.}/capsule/config.toml"
else
  capsule_config="$HOME/.config/capsule/config.toml"
  [[ -f "$capsule_config" ]] || capsule_config="$HOME/.capsule/config.toml"
fi
[[ "$capsule_config" == /* ]] || capsule_config="$PWD/$capsule_config"
printf '%s\n' "$capsule_config" > "$capsule_backup/config-path"
if [[ -f "$capsule_config" ]]; then
  cp "$capsule_config" "$capsule_backup/config.toml"
fi
printf 'Saved installation: %s\n' "$capsule_backup"
```

Keep the printed backup directory. The script follows the [configuration lookup](extending.md#file-and-examples) and records an absolute path for restoration. If the old binary is unavailable, obtain the previous package/version before proceeding.

## Retire the old service

For an imperatively installed service, use the saved old CLI before upgrading:

```zsh
"$capsule_backup/capsule" daemon uninstall
```

If uninstalling through the old CLI is unavailable, verify the service is imperatively managed and use the matching commands:

macOS:

```zsh
launchctl print "gui/$(id -u)/com.github.shuymn.capsule"
launchctl bootout "gui/$(id -u)/com.github.shuymn.capsule"
mv "$HOME/Library/LaunchAgents/com.github.shuymn.capsule.plist" "$capsule_backup/"
```

Linux:

```sh
systemctl --user disable --now capsule.socket
systemctl --user stop capsule.service
```

Keep disabled systemd unit files for rollback. Stop a foreground daemon in its owning terminal. Confirm the Capsule service has stopped.

For Home Manager, NixOS, or nix-darwin, remove `programs.capsule.daemon`, keep `programs.capsule.enable = true`, and deploy through the existing Nix owner. Move daemon environment entries into the shell's exported environment. Set `programs.capsule.enableZshIntegration = false` if another configuration owns initialization. Manage service files through Nix.

## Convert the configuration

Retain one top-level `schema_version = 2` and preserve display sections, styles, and connectors. Remove `[timeout]`, `[cache]`, and legacy `CAPSULE_SOCKET_PATH` / `CAPSULE_SOCK_DIR` exports.

Replace `[[module.source]]` blocks with `[module.values]` arrays. Group sources by `name`, use `value` for an omitted name, and preserve candidate order:

```toml
schema_version = 2

[[module]]
name = "cloud"
format = "{region}"

[module.values]
region = [
  { env = "AWS_REGION" },
  { env = "AWS_DEFAULT_REGION" },
]
```

Validate the converted document with `ConfigPlan::parse` and an isolated zsh home/config, using the [current value and reload rules](extending.md). Install the new binary, retain exactly one `eval "$(capsule init zsh)"` initialization, and start a new shell.

WHEN migration completes, the new shell SHALL show the intended prompt information with one session worker and no shared daemon. WHEN that shell exits or executes `exec zsh`, its worker SHALL stop.

## Roll back

Restore the configuration before starting the old runtime. If there was no old file, preserve the new configuration in the backup:

```zsh
# Set this to the directory printed during backup.
capsule_backup='/absolute/path/to/saved-installation'
capsule_config=$(cat "$capsule_backup/config-path")
if [[ -f "$capsule_backup/config.toml" ]]; then
  cp "$capsule_backup/config.toml" "$capsule_config"
elif [[ -f "$capsule_config" ]]; then
  mv "$capsule_config" "$capsule_backup/config-v2.toml"
fi
```

For a previously imperative installation:

```zsh
export PATH="$capsule_backup:$PATH"
rehash
capsule --version
capsule daemon install
exec zsh
```

Keep the saved binary first on `PATH` in subsequent shells until the package manager restores the old version. For Nix-managed installations, restore the matching old flake revision/deployment through its Nix owner, then start a new shell. Confirm the restored version and prompt before discarding either configuration.
