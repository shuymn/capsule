# capsule

[English](README.md)・日本語

入力を待たせない zsh のプロンプトです。`capsule` は Rust 製で、macOS と Linux で動きます。シェルごとにバックグラウンドの worker を1つ持ち、Git やツールの情報を集めている間も zsh は入力を受け付けます。

<p align="center">
  <img src="assets/vhs/readme-prompt.gif" alt="capsule プロンプトデモ">
</p>

## capsule を選ぶ理由

- **入力が止まりません。** `precmd` ですべてを計算するプロンプトでは、Enter のたびに `git status` を待つことになります。capsule はプロンプトをすぐに描画し、ディレクトリと環境が変わっていなければ前回確定した情報を表示したまま、新しい結果が届いた時点で差し替えます。
- **ツール専用のモジュールを持ちません。** Node、AWS、Kubernetes なども、環境変数・ファイル・コマンドの出力を読む数行の設定で表示できます。
- **zsh 専用であることは意図的な選択です。** 対象のシェルを1つに絞ることで、最小公倍数ではなく zsh の hook と行エディタを前提にした統合にしています。
- **表示するデータはデータのまま扱います。** ブランチ名、ファイルの内容、コマンドの出力はそのまま表示し、プロンプトのエスケープやシェルのコードとして展開しません（[脅威モデル](docs/threat-model.md)）。

## 計測結果

大きなリポジトリ（追跡ファイル 50,000、変更 200、未追跡 200）で、別の端末からブランチを切り替えた直後に空の行で Enter を押したときの値です。どちらのツールも既定の設定で、GitHub-hosted runner 上でツールごとに交互に 50 回計測した p50 / p90 を示します。

| Runner | ツール | 入力が止まる時間 | 新しいブランチの表示まで |
|---|---|---|---|
| ubuntu-latest（AMD EPYC、4 vCPU） | capsule 1.1.1 | 1.4 / 1.4 ms | 53 / 58 ms |
| | starship 1.26.0 | 73 / 79 ms | 73 / 79 ms |
| macos-latest（Apple M1 VM、3 vCPU） | capsule 1.1.1 | 1.7 / 2.4 ms | 82 / 111 ms |
| | starship 1.26.0 | 126 / 247 ms | 126 / 247 ms |

「入力が止まる時間」は zsh が再び入力を受け付けるまでの時間で、素の zsh プロンプトでは 0.4 ms です。「新しいブランチの表示まで」は新しいブランチ名が画面に出るまでの時間です。同じ runner 上でツールを比べた値であり、絶対的な性能を示すものではありません。[全結果](docs/benchmarks/2026-10-17-v1.1.1.md)と[計測方法](docs/benchmarking.md#user-visible-prompt-latency)を参照してください。Prompt latency workflow で誰でも再計測できます。

## プロンプト

```text
<directory> on <git branch> [indicators] via <custom value> took <duration>
<optional custom values> at <optional time> ❯
```

2行のプロンプトにディレクトリ、Git、カスタム値、コマンド実行時間、任意の時刻を表示します。プロンプト文字は直前のコマンドの成否を表し、vi コマンドモードでは `❮` に変わります。

## インストール

要件は macOS または Linux と zsh です。Git の表示には、シェルが export した `PATH` 上に `git` が必要です。

```sh
brew install shuymn/tap/capsule
# または
mise use -g github:shuymn/capsule
```

`.zshrc` に次を追加します。

```zsh
eval "$(capsule init zsh)"
```

0.x（共有 daemon 版）から更新する場合は、バイナリを置き換える前に[移行手順](docs/migration.md)を実施してください。

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

設定ファイルがなくても動きます。`capsule preset` で[編集できる設定例](examples/config.toml)を出力し、必要なモジュールを残して `~/.config/capsule/config.toml` に保存してください。次のモジュールは、プロジェクト内でだけ Node のバージョンを表示します。

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

取得元、表示条件、書式、スタイル、再読み込みについては[設定ガイド](docs/extending.md)を参照してください。

## スコープ

capsule は1人で保守できる規模を保ちます。特定ツールの表示は新しいモジュールではなく設定で書き、zsh 以外のシェルは対象外です。

## 開発

stable Rust と Task を使います。テストには uv、zsh、Git も必要です。実行時の契約は[アーキテクチャ](docs/architecture.md)、検証は[開発ツール](docs/tooling.md)、計測は[ベンチマーク](docs/benchmarking.md)、公開は[リリース手順](docs/releasing.md)を参照してください。
