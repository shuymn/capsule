# capsule

[English](README.md)・日本語

`capsule` は macOS と Linux で動作する、Rust 製の zsh 用プロンプトエンジンです。各シェルが1つの worker を持ち、プロンプトを更新している間も zsh は入力を受け付けます。

<p align="center">
  <img src="assets/vhs/readme-prompt.gif" alt="capsule プロンプトデモ">
</p>

## プロンプト

```text
<directory> on <git branch> [indicators] via <custom value> took <duration>
<optional custom values> at <optional time> ❯
```

2行のプロンプトにディレクトリ、Git、カスタム値、コマンド実行時間、任意の時刻を表示します。プロンプト文字は直前のコマンドの成否を表し、vi コマンドモードでは `❮` に変わります。カスタムモジュールは環境変数、ファイル、コマンド出力から値を取得します。

## インストール

要件は macOS または Linux と zsh です。Git の表示には、シェルが export した `PATH` 上に `git` が必要です。

旧 daemon 版から更新する場合は、バイナリを置き換える前に[移行手順](docs/migration.md)を実施してください。

```sh
brew install shuymn/tap/capsule
```

`.zshrc` に次を追加します。

```zsh
eval "$(capsule init zsh)"
```

### Nix

```sh
nix run github:shuymn/capsule -- --version
nix profile install github:shuymn/capsule
```

profile で導入する場合は `.zshrc` の設定を追加してください。宣言的な module は、バイナリの導入と zsh の初期化を行います。

```nix
# flake inputs に inputs.capsule.url = "github:shuymn/capsule" を追加
{
  imports = [ inputs.capsule.homeManagerModules.default ];
  programs.capsule.enable = true;
}
```

NixOS では `inputs.capsule.nixosModules.default`、nix-darwin では `inputs.capsule.darwinModules.default` を使用します。初期化を管理する module は1つにしてください。手動で初期化する場合は `programs.capsule.enableZshIntegration = false` にします。

## 設定

設定ファイルがなければ既定値を使います。`capsule preset` で [schema-v2 の設定例](examples/config.toml)を出力し、必要なモジュールを残して `~/.config/capsule/config.toml` に保存してください。

XDG パス、取得元、表示条件、書式、スタイル、再読み込みについては[設定ガイド](docs/extending.md)を参照してください。

## 開発

stable Rust と Task を使います。実行時の契約は[アーキテクチャ](docs/architecture.md)、検証は[開発ツール](docs/tooling.md)、公開は[リリース手順](docs/releasing.md)を参照してください。
