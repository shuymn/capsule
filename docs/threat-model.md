# 脅威モデル

シェル統合、設定探索、取得処理、表示、配布の信頼境界を変更する前に本書を読め。[architecture.md](architecture.md) の実行時契約と併用し、コード変更時は該当する脅威・対策・検証項目を更新せよ。

## 1. 対象と証拠の扱い

- 基準実装: `280378d718d9b61a16ef02b5cd9c350ff6b94e2f` に prompt 引用と PTY 回帰の変更を加えたもの、workspace version `1.0.3`、schema v2、macOS / Linux の対話的 zsh とセッション単位の worker。静的確認を始めた `6d84750` からの上流差分は依存と release workflow の更新のみである。
- 対象: `init zsh` / `preset`、内部の `worker` / `fd-config`、シェルと worker のパイプ、設定・環境・ファイル・Git・コマンドの取得、プロンプト表示、Nix 統合、CI とリリースの配布経路。
- 対象外: 旧共有 daemon / socket の実装、OS・zsh・端末・Git 自体の脆弱性調査、外部パッケージ管理基盤の監査、ベンチマーク・開発用スクリプトの個別監査。旧サービスの停止は [migration.md](migration.md) に従え。
- 根拠は設計資料とソースの静的確認、および第8節の検証記録である。検証記録にない項目はテストの所在だけを記録したもので、攻撃再現と外部設定の確認は実施していない。
- 「現行対策」はソースで確認できる実装、「前提」は実行環境が満たす条件、「残存リスク」は対策後も成立し得る経路として区別せよ。脅威シナリオを確認済み脆弱性や重大度判定として扱うな。

## 2. 資産、主体、前提

### 保護対象

| ID | 資産 | 保護する性質 |
| --- | --- | --- |
| A1 | シェル利用者の実行権限、ホーム内ファイル、ネットワーク・認証エージェントへのアクセス | 表示用データをコードに昇格させないこと。設定で意図した実行権限を無自覚に他者へ委譲しないこと |
| A2 | exported 環境、認証情報、設定内容、読み取り可能なファイル | 意図しない表示・診断・子プロセス・配布ログへの漏えいを避けること |
| A3 | プロンプト、入力バッファ、カーソル、シェルオプション、世代と観測結果の対応 | 制御列・プロンプト展開・別世代の応答で改変しないこと |
| A4 | 対話操作の応答性、worker と取得処理のリソース | 遅い取得、過大入力、子孫プロセス、壊れた通信でシェルを巻き込まないこと |
| A5 | ソース、candidate SHA、タグ、配布バイナリ、checksum、attestation、公開権限 | レビュー対象と配布物を対応させ、権限・成果物の差し替えを防ぐこと |

### 主体と能力

| 主体 | 能力と信頼の置き方 |
| --- | --- |
| シェル利用者・設定管理者 | バイナリ、zsh hooks、環境、設定、コマンドを選択する信頼主体。設定を採用することはコマンド実行権限の委譲を含む |
| 作業ディレクトリ・リポジトリの提供者 | ファイル名、ディレクトリ名、branch、ファイル内容、marker、symlink、提供された Git 設定を操作し得る。設定されたコマンドの起動条件や fallback を誘導し得る。表示データの提供者として扱い、実行権限は自動的に信用するな |
| 外部コマンド・Git とその helper | 利用者と同じ UID で実行され、exported 環境と通常の OS 権限にアクセスする。stdout は表示用データとして信用せず、実行ファイルの選択と副作用は別途信用する必要がある |
| ローカルの別 UID の利用者 | 共有・書き込み可能なディレクトリの内容を操作し得る。パイプへのアクセスは OS の descriptor 保護に依存する |
| 依存・Actions・配布基盤の提供者、リポジトリ権限保有者 | ビルド入力、CI 実行、タグ、公開物、パッケージを変更し得る。レビューと公開権限の境界で扱う |

前提を次のように限定せよ。

- 通常の利用者権限で実行する。setuid、特権サービス、複数利用者の共有 worker、root shell 向けの隔離を保証しない。
- OS、zsh、端末、採用した Capsule バイナリ、利用者が承認した hooks を信頼する。同一 UID の任意コード実行や root によるシェル・メモリ・descriptor の改変を防ぐ境界は設けていない。ただし、未信頼リポジトリのデータを読むだけで、その提供者に同一 UID の権限を与えたとみなすな。
- 設定ファイル、その親ディレクトリ、symlink の解決先、実行ファイルの探索経路は利用者の管理下にあることを要求する。Capsule は所有者・permission・出所を検証しない。
- 現行の runtime は継承パイプのみを使い、ネットワーク listener や共有 socket を公開しない。認証・暗号化はプロトコルの機能ではなく、OS の descriptor 所有に依存する。
- 外部コマンドのファイルアクセス・通信は制限されない。「Capsule 本体にネットワーク機能がない」を「秘密を外部送信できない」と読み替えるな。

## 3. データフローと信頼境界

```text
採用したバイナリ ── init zsh のスクリプト ── eval ──> 対話的 zsh
                                                        │
                       hooks 後の cwd / exported env / 世代 / local inputs
                                                        │ B1: 継承パイプ Q
                                                        ▼
                    セッション worker: reload / acquisition / display state
                      │ B2: ファイル・設定       │ B5: 実行権限の委譲
                      ▼                         ▼
        config / cwd のファイル / .git       Git / 設定した argv
                      │                         │ stdout
                      └──────────┬──────────────┘
                                 ▼ B3: データから表示への変換
                     View: sanitize → 幅調整 → style / % serialization
                                 │ B1: 継承パイプ R / K
                                 ▼ B4: zsh の prompt 展開
                      世代照合 → PROMPT 設定 → 端末 / scrollback

ソース / Cargo・Nix 依存 / Actions → CI → candidate / tag → archive / attestation
                                      └── B6: 公開・インストール → 採用したバイナリ
```

| 境界 | 入口と信頼上の要点 | 実装の根拠 |
| --- | --- | --- |
| B1 | 一つのシェルと一つの worker。`Q` は cwd・全 exported 環境と local inputs、`R` はシリアライズ済み prompt、`K` は書き込み再開 credit。世代番号は鮮度識別であり認証情報ではない | [init.zsh](../crates/core/src/init/init.zsh): `_capsule_start_coproc`, `_capsule_snapshot`, `_capsule_async_callback`; [session.rs](../crates/protocol/src/session.rs): `Request::decode`, `FrameReader`; [pipe.rs](../crates/cli/src/pipe.rs): `Pipe` |
| B2 | 設定の選択は exported `XDG_CONFIG_HOME` / `HOME`、file source は cwd 相対、Git metadata は祖先の `.git` から取得する。パス文字列の検証と filesystem の封じ込めを区別する | [worker.rs](../crates/cli/src/worker.rs): `config_paths`, `load_plan`; [compile.rs](../crates/core/src/plan/compile.rs): `validate_path`; [acquire.rs](../crates/core/src/acquire.rs): `Runner::file`; [git.rs](../crates/core/src/git.rs): `find_repository` |
| B3 | env・file・stdout・directory・Git・設定の表示文字列を、制御列や style ではなくデータとして扱う。View は I/O を行わない | [view.rs](../crates/core/src/view.rs): `render`, `evaluate_format`; [text.rs](../crates/core/src/view/text.rs): `StyledText::new`, `sanitize`, `truncate`; [style.rs](../crates/core/src/render/style.rs): `Style::paint_with` |
| B4 | trusted worker の応答のみを受け、zsh の `%` 展開と `PROMPT_SUBST` に対して文字列を literal のまま表示する | [init.zsh](../crates/core/src/init/init.zsh): `_capsule_frame`, `_capsule_apply_prompt`, `_capsule_refresh_retained_prompt`, `_capsule_run_precmd_hooks` |
| B5 | `Command::new` に argv を渡し、snapshot の cwd と全 exported 環境で実行する。implicit shell はないが OS sandbox もない | [acquire.rs](../crates/core/src/acquire.rs): `Runner::command`, `OwnedCommand`; [plan/acquire.rs](../crates/core/src/plan/acquire.rs): `acquire_condition`, `acquire_value` |
| B6 | ビルド入力・レビュー済み SHA・公開権限・配布物・インストールした executable の同一性を管理する | [releasing.md](releasing.md); [release-promote.yml](../.github/workflows/release-promote.yml); [release.yml](../.github/workflows/release.yml); [nix/package.nix](../nix/package.nix) |

設定は既定で `~/.config/capsule/config.toml`、次に `~/.capsule/config.toml` を探索する。exported `XDG_CONFIG_HOME` があればその値だけを使い、相対値は cwd 基準、空値は cwd 内の `capsule/config.toml` になる。`HOME` の相対値も cwd 基準になる。任意のリポジトリ設定を無条件に自動探索する仕様ではないが、環境によって設定の信頼主体が変わり得る。[extending.md](extending.md) と T03 を参照せよ。

## 4. 維持するセキュリティ条件

以下を EARS の受け入れ条件として使用せよ。隔離・機密保持について現行仕様が保証しない条件を追加する場合は、先に設計判断を更新せよ。

- **G1 — 表示を実行しない:** WHEN 未信頼の文字列を prompt に組み込む、Capsule SHALL 制御文字・対象の双方向制御文字を可視データに変換し、`%` を引用し、`PROMPT_SUBST` の on/off と hooks のエラーをまたいでも `$()`・backtick・変数参照を再評価しない。
- **G2 — 実行入力の忠実性:** WHEN 設定したコマンドまたは Git を起動する、Capsule SHALL direct argv と snapshot の cwd・exported 環境を使い、snapshot にない worker 環境の変数を子プロセスへ継ぎ足さない。これは実行先の安全性や秘密の最小開示を保証する条件ではない。
- **G3 — 有界な受け入れ:** WHEN frame・source・設定が上限を超える、Capsule SHALL 上限超過を拒否し、環境 snapshot を途中で切って実行しない。WHEN 新設定の検証に失敗する、Capsule SHALL 新設定全体を不採用にし、最後の valid plan を保持する。
- **G4 — セッションと世代:** WHEN 応答の世代が現在のシェル世代と異なる、Capsule SHALL prompt を変更しない。WHEN cwd・exported 環境・採用済み設定が変わる、Capsule SHALL 互換性のない retained display を破棄する。
- **G5 — 対話と終了:** WHILE 取得またはパイプ I/O が未完了である、Capsule SHALL 入力を受け付け、再描画時に入力バッファとカーソルを維持する。WHEN 所有する command が完了・取消・timeout・出力超過となる、Capsule SHALL process group の cleanup と直接 child の reap を行う。明示的に group / session を離脱した子孫は保証対象外とする。
- **G6 — 配布の対応:** WHEN 正規の promotion を行う、release workflow SHALL merged `main` 向け PR の candidate SHA の到達可能性と release contract を確認してからタグを作成する。WHEN 新規または draft release を公開する、release workflow SHALL 必須 archive・checksum・attestation の生成成功と checksum 照合を前提とする。

## 5. 脅威シナリオ

STRIDE を分類ラベルとして使う。`E` は root への昇格だけでなく、データ提供者が利用者の実行権限を得る経路も含む。分類から重大度や再現済みという結論を導くな。

### T01 — 表示データからの shell / terminal injection（T / E、B3・B4、A1・A3）

- **主体・条件:** リポジトリ提供者または source の出力提供者が、directory・branch・file・env・stdout に `$()`、backtick、`%{...%}`、ESC / OSC、改行、双方向制御文字を含める。
- **想定影響:** prompt 展開時のコード実行、端末操作、表示偽装、入力位置の破壊。
- **現行対策:** `StyledText` が controls と対象 Unicode 制御文字を `escape_default` で可視化する。`Style::paint_with` は `%` を `%%` に、`$`・backtick・`\` を `%1<<\$%<<` の形に引用する。`PROMPT_SUBST` が有効なら置換が backslash を除き、無効なら一桁の truncation が backslash を隠すため、どちらの option で展開しても同じ literal を表示する。style はデータと分離し、幅調整後に生成する。shell も `PROMPT_SUBST` 有効時に一段の `${_CAPSULE_RENDERED}` 参照を使い、保存した `precmd_functions` hooks の `always` 節で retained prompt を最終的な option に合わせる（B3・B4 の実装）。
- **確認した経路:** 名前付き `precmd()` は `precmd_functions` より先に実行され、genuine shell error を起こすと Capsule の hooks をすべて飛ばす。その後の prompt 展開は `PROMPT_SUBST` 無効時に代入した retained prompt をそのまま使い、エラー後に届く一致応答もエラー前に取得した option で代入される。この hook が option を有効化して失敗した後も、両経路で file source の `$()` と backtick が literal 表示のまま marker を作らないことを macOS / Linux の PTY で確認した（第8節）。引用は option の状態にも hook の順序にも依存しない。配列の先頭へ直接追加した hook、`periodic` / `sched`、ZLE widget が option を変えた後の展開も同じ性質で守るが、個別の PTY 再現はしていない。
- **残存リスク・扱い:** shell は応答を再 sanitize せず worker を信用する。worker / バイナリ / hook の侵害はこの対策で防げない。引用は zsh の prompt truncation と `PROMPT_SUBST` の backslash 処理に依存し、`PROMPT_PERCENT` を無効にした shell では style と同様に表示が崩れる（評価はされない）。Unicode の同形異字や通常の嘘の表示を全面的に防がず、切り詰めで重要な context が見えなくなることもある。prompt を認証・認可の根拠に使うな。

### T02 — executable 選択と条件・fallback を介した実行（S / E / I、B5、A1・A2）

- **主体・条件:** 提供者が cwd 内の executable、書き込み可能な `PATH` 要素、marker、先行 candidate のファイルを操作する。利用者が該当する command source を設定している。
- **想定影響:** 想定と異なるプログラムが prompt 更新のたびに実行される。正規のプログラムでも cwd の設定・plugin を読み、副作用や秘密へのアクセスを行い得る。
- **現行対策:** argv は設定から直接渡し、file / env / stdout を shell command に補間しない。condition が true と確定するまでは module の値を取得しない。candidate は順次試し、空文字を含む最初の成功で止める（`Runner::command`, `acquire_condition`, `acquire_value`）。
- **残存リスク・扱い:** condition は認可ではない。marker や先行 source の欠落・失敗で設定済みの後続 command が起動する。absolute executable は PATH 差し替えを減らすが plugin / cwd 由来の実行までは防がない。`sh -c` 等を明示すれば shell 実行も可能になる。preset を採用する前に command の全候補を確認し、未信頼 cwd で安全に実行できない module を除外せよ。

### T03 — 設定の出所・reload の誤認（T / E、B2・B5、A1・A3）

- **主体・条件:** 相対・空の `XDG_CONFIG_HOME`、相対 `HOME`、書き込み可能な設定経路、symlink、未確認の設定コピーを通じて提供者の TOML が選択される。
- **想定影響:** schema-valid な攻撃者指定 argv が利用者権限で実行される。無効な編集で実行を止めたつもりでも、最後の valid plan の command が残る。
- **現行対策:** 64 KiB 以内の regular file を読み、UTF-8・schema v2・unknown fields・source・regex・format・件数を検証する。parse は I/O や command 実行を行わず、失敗した新 plan は全体を不採用にする（`load_plan`, `ConfigPlan::parse`）。
- **残存リスク・扱い:** validation は構文・資源検証であり信頼性検証ではない。所有者・permission・symlink・信頼済み root は確認しない。無効 reload 時の last-valid retention は可用性の契約であって、実行停止の fail-closed policy ではない。管理された絶対 config path を使い、停止時は valid な設定で command を除去して採用を確認するか、対象 shell / worker を終了せよ。

### T04 — Git の外部処理と環境による誘導（E / I / D、B2・B5、A1・A2・A4）

- **主体・条件:** 提供された repository の local Git 設定や利用者の `GIT_*` 環境が、起動する Git の動作を変える。Git バージョンと設定によっては `core.fsmonitor` 等の外部 helper を起動し得る。
- **想定影響:** branch 表示のための `git status` がコード実行・秘密アクセス・過大な処理へ進む。表示する repository と cwd の実態が異なる場合もある。
- **現行対策:** `git --no-optional-locks status --porcelain=v2 --branch --show-stash` の固定 argv を共通 Runner で実行し、失敗時は Git 表示を出さない。cwd の祖先探索は 128 段までで、metadata 読み取りにも Runner の上限を使う（`acquire_context`, `find_repository`）。
- **残存リスク・扱い:** `--no-optional-locks` は sandbox / helper 無効化ではない。Git 設定・環境を隔離せず、組み込み Git も全 exported 環境を受け取る。実際の helper 起動条件は導入先の Git と設定で確認せよ。未信頼 repository でコード実行を許さない運用では `git.disabled = true` とし、custom commands も別に確認せよ。

### T05 — symlink・Git metadata による読み取り範囲の拡大（I / T、B2、A2・A3）

- **主体・条件:** 提供者が設定済み file source の相対パスを cwd 外への symlink に差し替える。または `.git` / `gitdir:` で metadata の読み取り先を誘導する。
- **想定影響:** 利用者が読める秘密ファイルの内容が module に表示され、端末・録画へ漏れる。file condition と実際の read の間の変更により、確認した marker と違う対象を読む。
- **現行対策:** source / condition のパスは absolute、`.`、`..`、NUL を拒否する。`Runner::file` は `RDONLY | CLOEXEC | NONBLOCK` で開いた descriptor の metadata が regular file であることを確認し、読み取りサイズを制限する。
- **残存リスク・扱い:** lexical path 制限は filesystem containment ではない。`NOFOLLOW` や directory-relative な root 封じ込めはなく、symlink を解決する。hardlink にも出所検証はない。`gitdir:` は absolute / relative の両方を受け入れる。condition は read と一体の認可検査ではない。未信頼 cwd の file source で機密保持を保証するな。symlink を制限する設計を採用する場合は、正規の worktree / symlink 利用との互換性を先に判断せよ。

### T06 — exported 環境・診断・表示からの秘密漏えい（I、B1・B4・B5、A2）

- **主体・条件:** 信用できない実行先にトークンや agent socket の参照を含む全 exported 環境を渡す。利用者が秘密の env / file / stdout を表示する module や、秘密を literal に含む設定を採用する。
- **想定影響:** 子プロセスによるファイルアクセス・外部送信、prompt / scrollback / screenshot / 録画 / crash dump からの開示。
- **現行対策:** shell snapshot は exported 変数のみを含み、`env_clear` 後にその snapshot を渡す。protocol / acquisition のエラーは取得した環境値を含めない。取得 command の stdin は EOF、stderr は破棄する（`_capsule_snapshot`, `Request::decode`, `Runner::command`）。
- **残存リスク・扱い:** `env_clear` は欠落変数の忠実性を守る処理で、allowlist や secret redaction ではない。snapshot は pipe buffer・shell / worker のメモリに残り、暗号化・zeroization は行わない。設定検証の診断は TOML / regex の内容を含み得るため「全診断から秘密が消える」と保証するな。秘密を prompt / 設定 literal に置かず、実行先を信用せよ。外部送信・dump・録画の制御は利用者環境の責任とする。

### T07 — 遅い I/O・過大入力・子孫による資源消費（D、B1・B2・B3・B5、A4）

- **主体・条件:** 過大 source、複雑な設定、大きな repository、応答しない filesystem、stdout を保持する子孫、頻繁な新世代、悪意ある command を与える。
- **想定影響:** worker の能力枯渇、prompt 情報の欠落、残存 process、ホスト全体の CPU / メモリ / disk / process 消費。
- **現行対策:** 第6節のサイズ・件数・並行数・timeout を使う。active generation と最新 pending replacement のみを持ち、cancel 済み blocking job / 遅延 reaper は完了まで permit を保持する。command 完了・取消・timeout・出力超過時に process group を kill し、直接 child を reap する。partial frame は完了または transport reset まで保持し、shell I/O は nonblocking とする。
- **残存リスク・扱い:** timeout は OS sandbox ではない。実行済みの副作用を戻せず、子孫の数・CPU・メモリ・書き込み・通信・他 process への signal を制限しない。group / session 離脱は cleanup 保証外。blocking filesystem call は deadline 後も残り得て、同時数を抑えても完了は保証しない。上限は worker ごとであり、shell 数に比例する。悪意ある command の実行を資源上限だけで許可するな。

### T08 — frame の破損・古い応答・セッション間混入（S / T / D、B1・B4、A2・A3・A4）

- **主体・条件:** 途中で切れた frame、backpressure、非 UTF-8 OS bytes、古い worker 応答、descriptor の漏えいによる偽応答が発生する。
- **想定影響:** 別世代の prompt を採用する、秘密 snapshot が別主体へ届く、I/O 待ちで入力が止まる。
- **現行対策:** 継承パイプを使い、shell endpoints を CLOEXEC / NONBLOCK にする。Rust は frame 上限、escaping、数値、環境 key の重複・NUL・`=` を検証し、unterminated EOF を拒否する。reader は取消をまたいで partial bytes を保持する。shell は応答長と世代の完全一致を確認し、通信停止時は fallback、次の command で worker を再起動する。
- **残存リスク・扱い:** shell 側の `R` parsing は trusted worker を前提としたもので、Rust の `Response::decode` 相当の全面検証や再 sanitize を行わない。`K` / 世代番号に MAC・nonce・peer 認証はない。descriptor へのアクセス権を持つ主体や同一 UID の任意コードを排除できない。将来 socket / shared worker に戻す場合は、この前提を流用せず認証・session 分離から再設計せよ。

### T09 — prompt の鮮度・真実性の過信（S / T、B2・B3・B4、A3）

- **主体・条件:** cwd・exported 環境・設定が同じまま file / Git / 外部結果を変更する。あるいは revalidation 中・失敗した reload 後の表示を利用者が最新だと誤認する。
- **想定影響:** branch、cloud profile、実行先、read-only 表示を誤信して次の操作を行う。
- **現行対策:** 同じ snapshot の settled display だけを取得中に保持し、fresh observations と混ぜない。cwd / exported 環境の変更時、異なる設定の採用時、transport failure 時に保持を破棄する。世代完了時は失敗・欠落・false condition も含めて外部表示を一括置換する（`State::new`, `State::finish`, `apply_change`）。
- **残存リスク・扱い:** prompt は継続監視でも atomic filesystem snapshot でもなく、取得中は以前の観測を意図的に表示する。無効 reload は以前の plan を使う。directory の read-only 表示は取得した permission の情報で、ACL・実際の操作可否の認可判定ではない。機密操作は実行直前に対象・権限を確認し、prompt を policy enforcement として扱うな。

### T10 — ビルド・公開経路の差し替え（S / T / E / I、B6、A1・A2・A5）

- **主体・条件:** 依存、Actions、toolchain、runner、公開 token、タグ作成権限、パッケージ経路のいずれかを侵害する。利用者は取得した `capsule init zsh` を `eval` する。
- **想定影響:** レビューと異なる binary / shell code の配布、利用者権限での実行、CI の秘密や公開権限の漏えい。
- **現行対策:** Cargo / Nix lock、Cargo の `--locked` build、直接参照する Actions / reusable workflows の commit SHA 固定、job ごとの permissions と checkout の `persist-credentials: false` を使う。[propose.sh](../scripts/release/propose.sh) は candidate commit の GitHub 署名を確認し、branch 更新前 SHA を指定して競合更新を防ぐ。promotion は merged `main` 向け PR の SHA と到達可能性、release contract、既存タグの一致を確認してから App token を作る。build が archive の attestation を生成し、公開 job は全 build 成功と必須 archive / checksum の照合を要求する。公開済み release の asset は workflow で上書きしない。
- **残存リスク・扱い:** lock / pin は承認済み入力の悪意を検出せず、stable toolchain・runner・action の内部取得物まで全て固定する保証でもない。本 workspace の `unsafe_code = forbid` は依存全体の安全性を証明しない。release は `v*` tag push で起動するため、promotion 外でのタグ作成防止は外部 ruleset と権限管理に依存する。checksum は独立した署名ではなく、attestation の生成と利用者側の検証は別である。公開済み asset の retry は名前の存在確認であり内容の再照合ではない。Nix cache / Homebrew の公開・消費も別経路として検証せよ。
- **初期化時の executable:** Nix modules は package の absolute path で `init` を呼ぶが、生成 script の `worker` / `fd-config` は `command capsule` を使う。以後の選択は shell の `PATH` に依存し、init の absolute path だけでは同一 binary を保証しない（[common.nix](../nix/modules/common.nix), [home-manager.nix](../nix/modules/home-manager.nix), `_capsule_start_coproc`）。

否認（R）に対する操作監査ログ・利用者識別は本製品のセキュリティ要件ではない。secret を含む snapshot を記録して埋め合わせるな。公開経路の追跡には candidate SHA・tag・workflow run・attestation を使い、利用者の shell 操作を監査したとみなすな。

## 6. 現行の資源上限

上限を変更するときは T07 と [architecture.md](architecture.md) を同時に更新せよ。数値をホスト全体の隔離保証として扱うな。

| 境界 | 上限 | 根拠 |
| --- | --- | --- |
| Escaped request / response（LF を除く） | 256 KiB / 64 KiB | `session.rs`: `MAX_REQUEST`, `MAX_RESPONSE`。shell snapshot は 261000 bytes で保守的に中止する |
| Config / 一つの取得値 | 各 64 KiB | `plan.rs`: `MAX_CONFIG_BYTES`; `acquire.rs`: `MAX_OUTPUT_BYTES` |
| Subprocess / blocking file job | worker ごとに 4 / 2 | `Runner::default`。worker runtime の blocking threads も 2 |
| 一つの I/O（capacity 待ち込み） / generation / cleanup grace | 500 ms / 2 s / 100 ms | `ACQUISITION_TIMEOUT`, `GENERATION_TIMEOUT`, `CLEANUP_TIMEOUT` |
| Modules / 全 values / 全 candidates | 64 / 256 / 1024 | `plan.rs` |
| Module ごとの values / value ごとの candidates | 16 / 8 | `plan.rs` |
| Condition の各リスト / command argv 要素数 | 各 64 / 64 | `compile.rs`: `MAX_WHEN_ITEMS`, `MAX_COMMAND_ARGS` |
| Format source / optional nesting | 4 KiB / 8 | `template.rs`, `plan.rs` |
| Compiled regex size / nesting | 64 KiB / 32 | `compile_candidate` |
| Module の format 展開 / prompt 一行の text | 64 KiB / 2 KiB | `view.rs`: `MAX_MODULE_TEXT_BYTES`; `text.rs`: `MAX_LINE_TEXT_BYTES`。後者は style・prompt 引用・wire escaping 前で、`\` は引用と wire escaping で 11 bytes になる |
| cwd 祖先の Git 探索 | 128 段 | `find_repository` |

## 7. 対応順と未決事項

以下は脆弱性 severity ではなく、信頼境界を維持するための作業順である。将来対策を「現行対策」に混ぜるな。

| 順序 | 脅威 | 現行仕様で要求する運用 / 回帰維持 | 未実装の検討事項 |
| --- | --- | --- | --- |
| 1: 実行権限 | T02・T03・T04・T10 | 設定の command 全候補、absolute config root、PATH、Git 設定、配布元を確認する。G1 の injection 回帰を維持する | init から worker までの executable 同一性を固定するか。未信頼 cwd の command / Git を禁止・制限する mode を設けるか |
| 2: 秘密 | T05・T06 | file source と全 exported 環境の権限委譲を確認し、秘密を表示・診断 fixture に含めない | file source を cwd に封じ込めるか、許可する symlink をどう定義するか。command ごとの env allowlist / sandbox を導入するか |
| 3: 通信・可用性 | T07・T08・T09 | frame / 世代 / timeout / cleanup / hook option の回帰を macOS・Linux で維持し、stale display を認可に使わない | OS レベルの資源隔離が必要か。worker 数、不可中断 I/O、鮮度をどう扱うか |
| 4: 公開基盤 | T10 | candidate と tag の対応、asset / attestation、secret scope と package 消費を検証する | 外部 ruleset・approval・attestation 検証の enforcement をどこで行うか |

[.github/infra.yaml](../.github/infra.yaml) は default branch の署名必須・PR / status check gate、release immutability、Actions の SHA 固定・read 既定権限・外部 fork 承認を宣言する。ただし approving review の必須数は `0` で、tag ruleset はこの宣言にない。宣言の適用済み状態や別途設定された保護は未確認である。promotion 自体は CI 成功を API で確認しないため、レビューと CI 待機の運用を自動 enforcement と混同するな。

ソース外の設定は未確認である。リポジトリ管理者に次を確認させ、証拠が得られるまで対策済みと記載するな。

- GitHub branch / tag rulesets と infra 宣言の適用状態、required CI、Release PR の review と merge 方法、workflow dispatch / caller / 実行 ref の権限、App の install scope / permissions、secret 保護、runner の信頼性。
- reusable Security checks workflow の実行内容と結果。ローカルの [security.yml](../.github/workflows/security.yml) に参照があることだけで、依存監査や秘密検出が通ったと断定するな。
- package 利用者が checksum と attestation の repository / workflow / commit identity を検証するか。Cachix の trust / key 管理と Homebrew / Maltmill の消費結果。
- 利用者の `PATH` / config root / symlink 解決先の権限、導入 Git の helper 設定、環境秘密・agent socket、端末録画・dump policy。外部基盤や実利用者データへの probe は行わず、管理者による確認を使え。

## 8. 検証計画と更新条件

以下の回帰を再利用せよ。実行済みとして扱えるのは「検証記録」に記載した結果だけである。追加検証は独立した一時 home / cwd、dummy 環境値、ローカル fixture で行い、実利用者の秘密・実サービスに触れるな。未信頼の実行対象は、外部 network を遮断し、空環境から明示 allowlist だけを設定し、source / toolchain を read-only、書き込み先を専用 scratch のみにした OS sandbox で実行せよ。CPU / memory / process / file size / disk / wall time に低い明示上限を設け、必要な制御が使えなければ実行せず未検証と記録せよ。再現に必要な command・dummy input・sandbox 上限・安全な allowlist 環境値のみを証拠に残せ。

| 脅威 / 条件 | 既存の検証先 | 追加確認・証拠 |
| --- | --- | --- |
| T01 / G1 | [text.rs](../crates/core/src/view/text.rs): `controls_are_display_data_before_width_calculation`; [view/tests.rs](../crates/core/src/view/tests.rs): `literal_data_is_safe_with_prompt_substitution_on_and_off`; [shell_contract.rs](../crates/cli/tests/shell_contract.rs): `retained_prompt_tracks_user_hook_option_changes_before_a_response`; [session_pty.py](../scripts/session_pty.py): `run_hook_errors`, `run_named_hook_error` | 保存した配列 hooks と、配列より先に動く名前付き `precmd()` を別々に検証し、実際の prompt 展開での literal 表示・dummy marker の副作用なし・入力保持を macOS / Linux で記録する。hook 未到達や timeout を成功扱いするな。PTY 手順は [testing.md](testing.md) |
| T02・T06 / G2 | [acquire/tests.rs](../crates/core/src/acquire/tests.rs): `command_preserves_raw_environment_and_clears_unspecified_variables`; [plan/tests.rs](../crates/core/src/plan/tests.rs): `ready_empty_env_stops_fallback_but_unset_env_does_not`, `missing_failed_and_timed_out_candidates_fall_back_in_order` | 信頼済み dummy executable だけで PATH / relative argv / marker 誘導を確認する。環境 fixture には秘密を使わない |
| T03 / G3 | [plan/tests.rs](../crates/core/src/plan/tests.rs): schema・source・regex・format・件数の validation; [worker_contract.rs](../crates/cli/tests/worker_contract.rs): `failed_reload_preserves_last_valid_plan_then_accepts_fixed_configuration`, `relative_config_paths_follow_the_request_cwd` | 相対・空・未設定 XDG と相対 HOME で選択 root を記録する。無効 reload は execution revoke ではないことも確認する |
| T04 | [git.rs](../crates/core/src/git.rs) と [git/tests.rs](../crates/core/src/git/tests.rs) | helper の無害な marker fixture で、導入する Git version ごとの外部起動条件を確認する。既存 Git 表示テストを helper 隔離の証拠に使わない |
| T05 | `validate_path`, `Runner::file`, `find_repository` の静的経路 | dummy の cwd 外ファイルへの symlink / hardlink、`gitdir:`、condition 後の差し替えで読み取り先を記録する。現行が拒否するという assertion を置かない |
| T07 / G3・G5 | [acquire/tests.rs](../crates/core/src/acquire/tests.rs): 出力上限、FIFO / 非 regular file、取消、子孫の stdout 保持、blocking permit; [view/tests.rs](../crates/core/src/view/tests.rs): `repeated_references_cannot_amplify_bounded_values`, `pathological_zero_width_data_still_fits_the_response_byte_limit`, `quoted_backslashes_on_both_lines_fit_the_response_byte_limit` | process group 内の cleanup と隔離された fixture の資源上限を確認する。group 離脱や停止しない filesystem は保証外として記録する |
| T08 / G4・G5 | [session.rs](../crates/protocol/src/session.rs): fragmentation / cancellation / EOF / frame limit / invalid env; [shell_contract.rs](../crates/cli/tests/shell_contract.rs): `responses_must_match_exact_generation`, partial reads; [scripts/session_pty.py](../scripts/session_pty.py) | real PTY で backpressure・worker failure・exit / exec・複数 shell を確認し、macOS / Linux の結果を別々に記録する |
| T09 / G4 | [worker_contract.rs](../crates/cli/tests/worker_contract.rs): `same_snapshot_retains_all_pending_frames_and_commits_external_changes_together`, `changed_environment_cwd_and_valid_config_clear_retained_information` | revalidation 中の保持と completion 時の失敗・欠落除去を確認する。filesystem の常時監視や atomic read の証拠としない |
| T10 / G6 | [scripts/release/check_test.sh](../scripts/release/check_test.sh)、[releasing.md](releasing.md) | local contract test と外部公開設定の管理者確認を分ける。異なる SHA の既存 tag の拒否、候補・archive・attestation の同一性を確認する。公開・tag push・外部権限変更は別途承認を得る |

### 検証記録

| 項目 | 内容 |
| --- | --- |
| ソース | `280378d` に本書と同時の prompt 引用・PTY 回帰の変更を加えた作業ツリー |
| 依存 | `Cargo.lock` は変更していない。registry の 101 package を lockfile の sha256 と照合して vendor 化し、`--locked` かつ offline でビルドした |
| macOS | arm64、zsh 5.9.1、rustc 1.96.0、Python 3.15。`sandbox-exec` で network を拒否し、PTY driver の `/bin/ps` だけを sandbox 外で起動した。資源上限は設けていない |
| Linux | aarch64 の container、zsh 5.9、rustc 1.99.0、Python 3.11。network なし、read-only root、全 capability の削除、`no-new-privileges`、memory 4 GiB、CPU 2、PID 192、書き込みは 3 GiB の tmpfs のみ、wall time 900 s |

| 検証 | macOS | Linux |
| --- | --- | --- |
| `run_named_hook_error` | 3 records 成功、marker なし | 同左 |
| `task fmt:check` / `lint` / `test` / `doc` / `build` | 成功 | 成功 |
| `scripts/session_pty.py` 全体 | 34 records 成功（`non_utf8_fixture_supported = false`） | 34 records 成功（`non_utf8_fixture_supported = true`） |

次の変更時は本書を更新せよ: CLI / IPC の公開面、共有 worker や socket の追加、設定探索・symlink policy、実行先・環境の選択、Git argv、sanitize / style / prompt 展開、retained display と generation、上限・cleanup、依存・Actions・公開経路の信頼方式。変更した境界には主体・入力・資産・影響・現行対策・残存リスク・検証先を必ず対応させよ。
