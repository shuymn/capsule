# Threat Model

Read this document before changing the trust boundaries of shell integration, config discovery, acquisition, rendering, or distribution. Use it together with the runtime contract in [architecture.md](architecture.md), and when changing code, update the affected threats, mitigations, and verification entries.

## 1. Scope and Treatment of Evidence

- Subject: schema v2, interactive zsh on macOS / Linux, and per-session workers.
- In scope: `init zsh` / `preset`, the internal `worker` / `fd-config`, the pipes between shell and worker, acquisition of config, environment, files, Git, and commands, prompt rendering, Nix integration, and the CI and release distribution path.
- Out of scope: vulnerability research on the OS, zsh, terminals, or Git themselves; audits of external package-management infrastructure; individual audits of benchmark and development scripts.
- Each current mitigation cites its implementation; section 8 maps each threat to its regressions. Its "Existing verification" column is generated from the threat tags on tests (see [testing.md](testing.md)); a threat marked "source review only" has no tagged regression.
- Keep "current mitigation" (implementation confirmed in the source), "assumption" (a condition the runtime environment satisfies), and "residual risk" (a path that may remain after mitigation) distinct. Do not treat a threat scenario as a confirmed vulnerability or a severity rating.

## 2. Assets, Actors, and Assumptions

### Protected assets

| ID | Asset | Property to protect |
| --- | --- | --- |
| A1 | The shell user's execution authority, files in the home directory, and access to the network and authentication agents | Never promote display data to code. Never delegate intended execution authority to others unknowingly through configuration |
| A2 | Exported environment, credentials, configuration contents, and readable files | Avoid leaks into unintended displays, diagnostics, child processes, and distribution logs |
| A3 | The prompt, input buffer, cursor, shell options, and the correspondence between generations and observations | Never alter them through control sequences, prompt expansion, or responses from another generation |
| A4 | Interactive responsiveness and the resources of the worker and acquisition | Never let slow acquisition, oversized input, descendant processes, or broken communication drag the shell down |
| A5 | Source, candidate SHA, tags, distributed binaries, checksums, attestations, and publishing permissions | Keep the reviewed subject and the distributed artifacts in correspondence, and prevent substitution of permissions or artifacts |

### Actors and capabilities

| Actor | Capability and trust |
| --- | --- |
| Shell user / configuration administrator | The trusted party who chooses the binary, zsh hooks, environment, configuration, and commands. Adopting a configuration includes delegating command-execution authority |
| Provider of the working directory / repository | May manipulate file names, directory names, branches, file contents, markers, symlinks, and supplied Git configuration. May steer the start conditions and fallbacks of configured commands. Treat as a provider of display data; never trust it with execution authority automatically |
| External commands, Git, and their helpers | Run under the user's UID and access the exported environment and ordinary OS permissions. Their stdout is not trusted and is treated only as display data, while the choice of executable and its side effects must be trusted separately |
| Another local user (different UID) | May manipulate the contents of shared or writable directories. Pipe access depends on OS descriptor protection |
| Providers of dependencies, Actions, and distribution infrastructure; holders of repository permissions | May alter build inputs, CI execution, tags, published artifacts, and packages. Handled at the boundary of review and publishing permissions |

Limit the assumptions as follows.

- Run with ordinary user privileges. Isolation for setuid, privileged services, workers shared by multiple users, or root shells is not guaranteed.
- Trust the OS, zsh, the terminal, the adopted Capsule binary, and hooks the user has approved. There is no boundary that prevents arbitrary code execution under the same UID or tampering with the shell, memory, or descriptors by root. However, do not treat merely reading untrusted repository data as granting its provider the same-UID authority.
- Require that the config file, its parent directories, symlink targets, and executable search paths be under the user's control. Capsule does not verify ownership, permissions, or provenance.
- The current runtime uses only inherited pipes and exposes no network listener or shared socket. Authentication and encryption are not protocol features; they depend on OS descriptor ownership.
- File access and communication by external commands are not restricted. Do not read "Capsule itself has no network functionality" as "secrets cannot be sent out."

## 3. Data Flow and Trust Boundaries

```text
adopted binary ── init zsh script ── eval ──> interactive zsh
                                                  │
                  cwd / exported env / generation / local inputs after hooks
                                                  │ B1: inherited pipe Q
                                                  ▼
               session worker: reload / acquisition / display state
                 │ B2: files, config          │ B5: delegated execution authority
                 ▼                            ▼
   config / files in cwd / .git        Git / configured argv
                 │                            │ stdout
                 └──────────┬─────────────────┘
                            ▼ B3: data-to-display conversion
                View: sanitize → width fitting → style / % serialization
                            │ B1: inherited pipes R / K
                            ▼ B4: zsh prompt expansion
                 generation match → PROMPT assignment → terminal / scrollback

source / Cargo and Nix dependencies / Actions → CI → candidate / tag → archive / attestation
                                                 └── B6: publish and install → adopted binary
```

| Boundary | Entry point and trust considerations | Implementation basis |
| --- | --- | --- |
| B1 | One shell and one worker. `Q` carries the cwd, all exported environment, and local inputs; `R` is the serialized prompt; `K` is the write-resume credit. The generation number identifies freshness and is not a credential | [init.zsh](../crates/core/src/init/init.zsh): `_capsule_start_coproc`, `_capsule_snapshot`, `_capsule_async_callback`; [session.rs](../crates/protocol/src/session.rs): `Request::decode`, `FrameReader`; [pipe.rs](../crates/cli/src/pipe.rs): `Pipe` |
| B2 | Config is selected from exported `XDG_CONFIG_HOME` / `HOME`, file sources are cwd-relative, and Git metadata comes from an ancestor `.git`. Path-string validation is distinct from filesystem containment | [worker.rs](../crates/cli/src/worker.rs): `config_paths`, `load_plan`; [compile.rs](../crates/core/src/plan/compile.rs): `validate_path`; [acquire.rs](../crates/core/src/acquire.rs): `Runner::file`; [git.rs](../crates/core/src/git.rs): `find_repository` |
| B3 | Treat display strings from env, file, stdout, directory, Git, and configuration as data, not as control sequences or style. View performs no I/O | [view.rs](../crates/core/src/view.rs): `render`, `evaluate_format`; [text.rs](../crates/core/src/view/text.rs): `StyledText::new`, `sanitize`, `truncate`; [style.rs](../crates/core/src/render/style.rs): `Style::paint_with` |
| B4 | Accept only responses from the trusted worker, and display strings literally despite zsh `%` expansion and `PROMPT_SUBST` | [init.zsh](../crates/core/src/init/init.zsh): `_capsule_frame`, `_capsule_apply_prompt`, `_capsule_refresh_retained_prompt`, `_capsule_run_precmd_hooks` |
| B5 | Pass argv to `Command::new` and run with the snapshot's cwd and all exported environment. There is no implicit shell, but also no OS sandbox | [acquire.rs](../crates/core/src/acquire.rs): `Runner::command`, `OwnedCommand`; [plan/acquire.rs](../crates/core/src/plan/acquire.rs): `acquire_condition`, `acquire_value` |
| B6 | Manages the identity of build inputs, reviewed SHAs, publishing permissions, distributed artifacts, and installed executables | [releasing.md](releasing.md); [release-promote.yml](../.github/workflows/release-promote.yml); [release.yml](../.github/workflows/release.yml); [nix/package.nix](../nix/package.nix) |

Config is searched by default at `~/.config/capsule/config.toml`, then `~/.capsule/config.toml`. If exported `XDG_CONFIG_HOME` is set, only that value is used; a relative value is resolved against the cwd, and an empty value yields `capsule/config.toml` under the cwd. A relative `HOME` is also resolved against the cwd. The specification does not unconditionally auto-discover arbitrary repository configs, but the environment can change who is the trusted party for configuration. See [extending.md](extending.md) and T03.

## 4. Security Conditions to Maintain

Use the following as EARS acceptance criteria. Before adding a condition about isolation or confidentiality that the current specification does not guarantee, update the design decision first.

- **G1 — Do not execute display data:** WHEN untrusted strings are embedded in the prompt, Capsule SHALL convert control characters and the targeted bidirectional control characters into visible data, quote `%`, and never re-evaluate `$()`, backticks, or variable references, regardless of `PROMPT_SUBST` being on or off or hook errors.
- **G2 — Fidelity of execution input:** WHEN launching a configured command or Git, Capsule SHALL use direct argv and the snapshot's cwd and exported environment, and SHALL NOT add worker-environment variables absent from the snapshot to the child process. This is not a condition guaranteeing the safety of the target or minimal disclosure of secrets.
- **G3 — Bounded acceptance:** WHEN a frame, source, or configuration exceeds its limit, Capsule SHALL reject it and SHALL NOT execute with a truncated environment snapshot. WHEN validation of a new configuration fails, Capsule SHALL reject the entire new configuration and keep the last valid plan.
- **G4 — Session and generation:** WHEN a response's generation differs from the shell's current generation, Capsule SHALL NOT change the prompt. WHEN the cwd, exported environment, or adopted configuration changes, Capsule SHALL discard any incompatible retained display.
- **G5 — Interaction and termination:** WHILE acquisition or pipe I/O is incomplete, Capsule SHALL keep accepting input and SHALL preserve the input buffer and cursor on redraw. WHEN a command it owns completes, is cancelled, times out, or exceeds its output limit, Capsule SHALL clean up the process group and reap the direct child. Descendants that explicitly leave the group / session are outside the guarantee.
- **G6 — Distribution correspondence:** WHEN performing a legitimate promotion, the release workflow SHALL verify the reachability of the candidate SHA of the merged PR targeting `main` and the release contract before creating the tag. WHEN publishing a new or draft release, the release workflow SHALL proceed only after confirming that the required archives, checksums, and attestations were generated successfully and that the checksums match.

## 5. Threat Scenarios

STRIDE is used as a classification label. `E` includes not only escalation to root but also paths by which a data provider gains the user's execution authority. Do not derive severity or "reproduced" status from the classification.

### T01 — Shell / terminal injection from display data (T / E, B3·B4, A1·A3)

- **Actor and conditions:** A repository provider or a source-output provider puts `$()`, backticks, `%{...%}`, ESC / OSC, newlines, or bidirectional control characters into a directory, branch, file, env, or stdout.
- **Expected impact:** Code execution during prompt expansion, terminal manipulation, spoofed display, and corruption of the input position.
- **Current mitigation:** `StyledText` makes controls and the targeted Unicode control characters visible via `escape_default`. `Style::paint_with` turns `%` into `%%` and quotes `$`, backtick, and `\` as `%1<<\$%<<`. With `PROMPT_SUBST` enabled, the substitution removes the backslash; with it disabled, the single-digit truncation hides the backslash, so either option setting displays the same literal. Style is kept separate from data and generated after width fitting. The shell also uses a single level of `${_CAPSULE_RENDERED}` reference when `PROMPT_SUBST` is enabled, and the `always` clause of the saved `precmd_functions` hooks reconciles the retained prompt with the final option (the implementation of B3·B4).
- **Paths covered:** A named `precmd()` runs before `precmd_functions`, and a genuine shell error in it skips all Capsule hooks. The subsequent prompt expansion uses the retained prompt assigned while `PROMPT_SUBST` was disabled, and a matching response arriving after the error is assigned under the option captured before the error. On both paths, file-source `$()` and backticks stay literal and create no marker even after the hook enables the option and then fails; `run_named_hook_error` in [hooks.py](../scripts/pty_suite/scenarios/hooks.py) covers this on macOS and Linux. Because the quoting depends on neither the option state nor hook order, it also protects prompt expansion after the option is changed by a hook added directly to the head of the array, `periodic` / `sched`, or a ZLE widget; those paths have no dedicated PTY regression.
- **Residual risk and handling:** The shell does not re-sanitize responses and trusts the worker. Compromise of the worker, the binary, or a hook is not prevented by this mitigation. The quoting depends on zsh's prompt truncation and `PROMPT_SUBST` backslash handling, and in a shell with `PROMPT_PERCENT` disabled the display breaks, as with style (but nothing is evaluated). Unicode homoglyphs and ordinary lying display are not fully prevented, and truncation can hide important context. Do not use the prompt as a basis for authentication or authorization.

### T02 — Execution via executable selection, conditions, and fallbacks (S / E / I, B5, A1·A2)

- **Actor and conditions:** A provider manipulates an executable in the cwd, a writable `PATH` element, a marker, or the file of a preceding candidate. The user has configured the relevant command source.
- **Expected impact:** An unintended program runs on every prompt update. Even a legitimate program may read configuration or plugins from the cwd, and may have side effects or access secrets.
- **Current mitigation:** argv is passed directly from configuration, and file / env / stdout are not interpolated into shell commands. A module's value is not acquired until its condition is determined to be true. Candidates are tried in order, stopping at the first success, including an empty string (`Runner::command`, `acquire_condition`, `acquire_value`).
- **Residual risk and handling:** A condition is not authorization. A missing or failing marker or preceding source can start a configured later command. An absolute executable reduces PATH substitution but does not prevent execution driven by plugins or the cwd. Explicitly specifying `sh -c` and the like also enables shell execution. Before adopting a preset, review all command candidates and exclude modules that cannot be run safely in an untrusted cwd.

### T03 — Misjudging config provenance and reloads (T / E, B2·B5, A1·A3)

- **Actor and conditions:** A provider's TOML is selected through a relative or empty `XDG_CONFIG_HOME`, a relative `HOME`, a writable config path, a symlink, or an unverified copy of the configuration.
- **Expected impact:** A schema-valid, attacker-specified argv runs with the user's privileges. Even when an invalid edit was meant to stop execution, the command from the last valid plan remains.
- **Current mitigation:** Reads a regular file of at most 64 KiB and validates UTF-8, schema v2, unknown fields, sources, regexes, formats, and counts. Parsing performs no I/O or command execution, and a failed new plan is rejected as a whole (`load_plan`, `ConfigPlan::parse`).
- **Residual risk and handling:** Validation checks syntax and resources, not trustworthiness. Ownership, permissions, symlinks, and trusted roots are not checked. Last-valid retention on an invalid reload is an availability contract, not a fail-closed policy that stops execution. Use a managed absolute config path; to stop execution, remove the command from a valid configuration and confirm its adoption, or terminate the target shell / worker.

### T04 — Git external processing and environment steering (E / I / D, B2·B5, A1·A2·A4)

- **Actor and conditions:** The local Git configuration of a supplied repository, or the user's `GIT_*` environment, changes the behavior of the Git that is launched. Depending on Git version and configuration, an external helper such as `core.fsmonitor` may be started.
- **Expected impact:** The `git status` run for branch display proceeds to code execution, secret access, or excessive processing. The displayed repository may also differ from the actual cwd.
- **Current mitigation:** Runs the fixed argv `git --no-optional-locks status --porcelain=v2 --branch --show-stash` through the shared Runner, and shows no Git display on failure. The ancestor search from the cwd goes at most 128 levels, and metadata reads also use the Runner's limits (`acquire_context`, `find_repository`).
- **Residual risk and handling:** `--no-optional-locks` is neither a sandbox nor helper disabling. Git configuration and environment are not isolated, and the built-in Git also receives all exported environment. Verify the actual helper launch conditions against the deployed Git and its configuration. In deployments that must not allow code execution in untrusted repositories, set `git.disabled = true` and review custom commands separately.

### T05 — Read-scope expansion through symlinks and Git metadata (I / T, B2, A2·A3)

- **Actor and conditions:** A provider replaces the relative path of a configured file source with a symlink pointing outside the cwd, or steers where metadata is read via `.git` / `gitdir:`.
- **Expected impact:** The contents of secret files readable by the user appear in a module and leak to the terminal or recordings. A change between a file condition and the actual read makes it read something other than the marker that was checked.
- **Current mitigation:** Source / condition paths reject absolute paths, `.`, `..`, and NUL. `Runner::file` opens with `RDONLY | CLOEXEC | NONBLOCK`, confirms that the descriptor's metadata is a regular file, and limits the read size.
- **Residual risk and handling:** The lexical path restriction is not filesystem containment. There is no `NOFOLLOW` or directory-relative root containment, and symlinks are resolved. Hardlinks also have no provenance check. `gitdir:` accepts both absolute and relative paths. A condition is not an authorization check bound to the read. Do not guarantee confidentiality for file sources in an untrusted cwd. If adopting a design that restricts symlinks, first decide its compatibility with legitimate worktree / symlink usage.

### T06 — Secret leakage from exported environment, diagnostics, and display (I, B1·B4·B5, A2)

- **Actor and conditions:** All exported environment, including tokens and references to agent sockets, is passed to an untrusted target. The user adopts a module that displays a secret env / file / stdout, or a configuration containing secret literals.
- **Expected impact:** File access and external transmission by child processes; disclosure through the prompt, scrollback, screenshots, recordings, or crash dumps.
- **Current mitigation:** The shell snapshot contains only exported variables, and that snapshot is passed after `env_clear`. Protocol / acquisition errors do not include acquired environment values. The stdin of acquisition commands is EOF and stderr is discarded (`_capsule_snapshot`, `Request::decode`, `Runner::command`).
- **Residual risk and handling:** `env_clear` preserves fidelity for missing variables; it is not an allowlist or secret redaction. The snapshot remains in pipe buffers and shell / worker memory, with no encryption or zeroization. Configuration-validation diagnostics may contain TOML / regex content, so do not guarantee that secrets vanish from all diagnostics. Do not place secrets in the prompt or configuration literals, and trust the execution target. Controlling external transmission, dumps, and recordings is the responsibility of the user's environment.

### T07 — Resource consumption by slow I/O, oversized input, and descendants (D, B1·B2·B3·B5, A4)

- **Actor and conditions:** Oversized sources, complex configurations, large repositories, unresponsive filesystems, descendants holding stdout, frequent new generations, or malicious commands are supplied.
- **Expected impact:** Exhaustion of worker capacity, loss of prompt information, leftover processes, and host-wide CPU / memory / disk / process consumption.
- **Current mitigation:** Uses the sizes, counts, concurrency, and timeouts in section 6. Holds only the active generation and the latest pending replacement, and cancelled blocking jobs / delayed reapers keep their permit until completion. On command completion, cancellation, timeout, or output overflow, it kills the process group and reaps the direct child. Partial frames are held until completion or transport reset, and shell I/O is nonblocking.
- **Residual risk and handling:** A timeout is not an OS sandbox. It cannot undo side effects already performed, and it does not limit descendant count, CPU, memory, writes, communication, or signals to other processes. Leaving the group / session is outside the cleanup guarantee. Blocking filesystem calls can outlive the deadline, and limiting concurrency does not guarantee their completion. The limits are per worker and scale with the number of shells. Do not permit running malicious commands on resource limits alone.

### T08 — Frame corruption, stale responses, and cross-session mixing (S / T / D, B1·B4, A2·A3·A4)

- **Actor and conditions:** Truncated frames, backpressure, non-UTF-8 OS bytes, stale worker responses, or forged responses through descriptor leakage occur.
- **Expected impact:** A prompt from another generation is adopted, a secret snapshot reaches another party, or input stalls while waiting on I/O.
- **Current mitigation:** Uses inherited pipes and makes shell endpoints CLOEXEC / NONBLOCK. Rust validates frame limits, escaping, numbers, and duplicate / NUL / `=` in environment keys, and rejects unterminated EOF. The reader keeps partial bytes across cancellation. The shell checks that response length and generation match exactly, falls back when communication stops, and restarts the worker at the next prompt.
- **Residual risk and handling:** The shell-side `R` parsing assumes a trusted worker and does not perform full validation equivalent to Rust's `Response::decode`, nor does it re-sanitize responses. `K` and generation numbers have no MAC, nonce, or peer authentication. It cannot exclude a party with descriptor access or arbitrary code under the same UID. If a socket / shared worker is reintroduced, do not reuse these assumptions; redesign from authentication and session separation.

### T09 — Overreliance on prompt freshness and truthfulness (S / T, B2·B3·B4, A3)

- **Actor and conditions:** Files / Git / external results change while the cwd, exported environment, and configuration stay the same; or the user mistakes a display shown during revalidation or after a failed reload for the latest one.
- **Expected impact:** The user mistakenly relies on the displayed branch, cloud profile, execution target, or read-only indicator when taking the next action.
- **Current mitigation:** Only the settled display of the same snapshot is held during acquisition, and it is not mixed with fresh observations. The hold is discarded on a cwd / exported-environment change, on adoption of a different configuration, and on transport failure. When a generation completes, external displays are replaced all at once, including failures, omissions, and false conditions (`State::new`, `State::finish`, `State::load`).
- **Residual risk and handling:** The prompt is neither continuous monitoring nor an atomic filesystem snapshot, and during acquisition it intentionally shows earlier observations. An invalid reload uses the previous plan. A directory's read-only indicator reflects acquired permission information, not an ACL or an authorization decision on whether an operation is actually possible. For sensitive operations, check the target and permissions immediately before acting, and do not treat the prompt as policy enforcement.

### T10 — Substitution of the build and publishing path (S / T / E / I, B6, A1·A2·A5)

- **Actor and conditions:** Any of dependencies, Actions, the toolchain, runners, publishing tokens, tag-creation permission, or the package path is compromised. The user `eval`s the fetched `capsule init zsh`.
- **Expected impact:** Distribution of binaries / shell code differing from what was reviewed, execution with the user's privileges, and leakage of CI secrets or publishing permissions.
- **Current mitigation:** Uses Cargo / Nix locks, Cargo `--locked` builds, commit-SHA pinning of directly referenced Actions / reusable workflows, per-job permissions, and `persist-credentials: false` on checkout. [propose.sh](../scripts/release/propose.sh) verifies the GitHub signature of the candidate commit and prevents concurrent updates by specifying the pre-update SHA of the branch. Promotion verifies the SHA and reachability of the merged PR targeting `main`, the release contract, and agreement with any existing tag before creating the App token. The build generates attestations for archives, and the publishing job requires all builds to succeed and the required archives / checksums to match. Assets of published releases are not overwritten by the workflow.
- **Residual risk and handling:** Locks / pins do not detect malice in approved inputs, nor do they guarantee pinning everything down to what the stable toolchain, runners, and actions fetch internally. This workspace's `unsafe_code = forbid` does not prove the safety of the entire dependency tree. Releases start on a `v*` tag push, so preventing tag creation outside promotion depends on external rulesets and permission management. A checksum is not an independent signature, and generating an attestation is separate from verifying it on the user side. Retrying published assets checks only name existence, not content re-verification. Verify the Nix cache / Homebrew publishing and consumption as separate paths.
- **Executable at initialization:** Nix modules call `init` by the package's absolute path, but the generated script's `worker` / `fd-config` use `command capsule`. Later selection depends on the shell's `PATH`, so the absolute path of init alone does not guarantee the same binary ([common.nix](../nix/modules/common.nix), [home-manager.nix](../nix/modules/home-manager.nix), `_capsule_start_coproc`).

Operation audit logs and user identification for repudiation (R) are not security requirements of this product. Do not compensate by recording snapshots that contain secrets. Use the candidate SHA, tag, workflow run, and attestation to trace the publishing path, and do not treat it as an audit of the user's shell operations.

## 6. Current Resource Limits

[limits.rs](../crates/core/src/limits.rs) defines every limit once; each row names its constant, and [docs.rs](../crates/cli/tests/docs.rs) fails when a row, value, or constant is missing on either side. Do not treat the numbers as a host-wide isolation guarantee; they apply per worker and scale with the number of shells.

| Boundary | Limit | Constant |
| --- | --- | --- |
| Escaped request frame, excluding LF | 256 KiB | `MAX_REQUEST` |
| Escaped response frame, excluding LF | 64 KiB | `MAX_RESPONSE` |
| Escaped shell snapshot; a larger one omits acquisition | 261000 bytes | `MAX_SNAPSHOT_BYTES` |
| One acquired command, file, or environment value | 64 KiB | `MAX_OUTPUT_BYTES` |
| Config file | 64 KiB | `MAX_CONFIG_BYTES` |
| Concurrent subprocesses | 4 | `MAX_PROCESSES` |
| Concurrent blocking file jobs, and the worker runtime's blocking threads | 2 | `MAX_FILE_JOBS` |
| Acquisition events queued for the worker loop before producers wait | 16 | `EVENT_QUEUE_CAPACITY` |
| One I/O operation, including the wait for capacity | 500 ms | `ACQUISITION_TIMEOUT` |
| Acquisition generation | 2 s | `GENERATION_TIMEOUT` |
| Process-group cleanup grace | 100 ms | `CLEANUP_TIMEOUT` |
| Draining cancelled generations after the worker stops serving | 1 s | `GENERATION_DRAIN_TIMEOUT` |
| Blocking tasks at worker runtime shutdown | 500 ms | `RUNTIME_SHUTDOWN_TIMEOUT` |
| Modules | 64 | `MAX_MODULES` |
| Values across all modules | 256 | `MAX_TOTAL_VALUES` |
| Candidates across all values | 1024 | `MAX_TOTAL_CANDIDATES` |
| Values per module | 16 | `MAX_VALUES_PER_MODULE` |
| Candidates per value | 8 | `MAX_CANDIDATES_PER_VALUE` |
| Entries in one condition list | 64 | `MAX_WHEN_ITEMS` |
| Command argv elements | 64 | `MAX_COMMAND_ARGS` |
| Format source | 4 KiB | `MAX_FORMAT_BYTES` |
| Optional format nesting | 8 | `MAX_FORMAT_DEPTH` |
| Compiled regex size | 64 KiB | `MAX_REGEX_BYTES` |
| Regex nesting | 32 | `MAX_REGEX_NESTING` |
| Expanded text of one module format | 64 KiB | `MAX_MODULE_TEXT_BYTES` |
| Plain text per prompt line, before style, prompt quoting, and wire escaping (`\` becomes 11 bytes) | 2 KiB | `MAX_LINE_TEXT_BYTES` |
| Cwd ancestors searched for a Git repository | 128 | `MAX_REPOSITORY_DEPTH` |

## 7. Priorities and Open Questions

The following is the order of work for maintaining the trust boundaries, not vulnerability severity. Do not mix future mitigations into "current mitigation."

| Order | Threats | Operation / regression coverage required by the current specification | Unimplemented considerations |
| --- | --- | --- | --- |
| 1: Execution authority | T02·T03·T04·T10 | Review all command candidates in the configuration, the absolute config root, PATH, Git configuration, and the distribution source. Keep the G1 injection regressions | Whether to pin executable identity from init through the worker. Whether to add a mode that forbids or restricts commands / Git in an untrusted cwd |
| 2: Secrets | T05·T06 | Review the authority delegated by file sources and all exported environment, and keep secrets out of display and diagnostic fixtures | Whether to contain file sources to the cwd, and how to define permitted symlinks. Whether to introduce a per-command env allowlist / sandbox |
| 3: Communication and availability | T07·T08·T09 | Keep the frame / generation / timeout / cleanup / hook-option regressions on macOS and Linux, and never use a stale display for authorization | Whether OS-level resource isolation is needed. How to handle worker count, uninterruptible I/O, and freshness |
| 4: Publishing infrastructure | T10 | Verify the candidate-to-tag correspondence, assets / attestations, secret scope, and package consumption | Where to enforce external rulesets, approvals, and attestation verification |

[.github/infra.yaml](../.github/infra.yaml) declares required signatures and PR / status-check gates on the default branch, release immutability, SHA pinning of Actions, read-only default permissions, and approval for external forks. However, the required number of approving reviews is `0`, and a tag ruleset is not in this declaration. The repository source does not establish whether the declaration is applied or which other protections exist. Promotion itself does not confirm CI success through the API, so do not confuse the review and CI-wait practice with automated enforcement.

The repository administrator owns the following configuration outside the source. Do not list any of it as a current mitigation without evidence from the administrator.

- The applied state of GitHub branch / tag rulesets and the infra declaration, required CI, the review and merge method for Release PRs, permissions for workflow dispatch / callers / execution refs, the App's install scope / permissions, secret protection, and runner trustworthiness.
- The contents and results of the reusable Security checks workflow. Do not conclude that dependency audits or secret detection passed merely because the local [security.yml](../.github/workflows/security.yml) references it.
- Whether package users verify the repository / workflow / commit identity of checksums and attestations. Cachix trust / key management and the consumption results of Homebrew / Maltmill.
- The permissions on the user's `PATH` / config root / symlink targets, the deployed Git's helper configuration, environment secrets and agent sockets, and terminal recording / dump policy. Do not probe external infrastructure or real user data; rely on confirmation by administrators.

## 8. Verification Plan and Update Conditions

Reuse the following regressions. Run additional verification with an independent temporary home / cwd, dummy environment values, and local fixtures, and never touch real user secrets or real services. Run untrusted execution targets in an OS sandbox with external network blocked, an empty environment with only an explicit allowlist set, source / toolchain read-only, and writes limited to a dedicated scratch. Set low explicit limits on CPU / memory / process / file size / disk / wall time, and do not run the check when the required controls are unavailable. Keep as evidence only the commands, dummy inputs, sandbox limits, and safe allowlisted environment values needed for reproduction.

| Threat / condition | Existing verification | Additional checks and evidence |
| --- | --- | --- |
| T01 / G1 | [crates/cli/tests/shell_contract.rs](../crates/cli/tests/shell_contract.rs): `retained_prompt_tracks_user_hook_option_changes_before_a_response`; [crates/core/src/view/tests.rs](../crates/core/src/view/tests.rs): `literal_data_is_safe_with_prompt_substitution_on_and_off`, `rendered_lines_fit_the_columns_and_the_response_frame`; [crates/core/src/view/text.rs](../crates/core/src/view/text.rs): `controls_are_display_data_before_width_calculation`, `sanitizing_is_idempotent_and_removes_controls`, `serialization_displays_the_plain_text_and_hides_escapes_in_style_sequences`; [scripts/pty_suite/scenarios/hooks.py](../scripts/pty_suite/scenarios/hooks.py): `run_hook_errors`, `run_named_hook_error` | Verify saved array hooks and a named `precmd()` that runs before the array separately, and record on macOS / Linux the literal display in real prompt expansion, no side effects of the dummy marker, and input preservation. Do not treat an unreached hook or a timeout as success. For PTY procedures see [testing.md](testing.md) |
| T02·T06 / G2 | [crates/core/src/acquire/tests.rs](../crates/core/src/acquire/tests.rs): `command_preserves_raw_environment_and_clears_unspecified_variables`; [crates/core/src/plan/tests.rs](../crates/core/src/plan/tests.rs): `ready_empty_env_stops_fallback_but_unset_env_does_not`, `missing_failed_and_timed_out_candidates_fall_back_in_order`; [crates/protocol/src/session.rs](../crates/protocol/src/session.rs): `invalid_frames_fail_without_exposing_values`, `rejected_frames_do_not_expose_environment_values` | Check PATH / relative argv / marker steering using only trusted dummy executables. Do not use secrets in environment fixtures |
| T03 / G3 | [crates/cli/tests/worker_contract.rs](../crates/cli/tests/worker_contract.rs): `failed_reload_preserves_last_valid_plan_then_accepts_fixed_configuration`, `relative_config_paths_follow_the_request_cwd`; [crates/core/src/plan/tests.rs](../crates/core/src/plan/tests.rs): `schema_and_unknown_fields_are_rejected_at_every_boundary`, `source_and_regex_validation_happens_before_acquisition`, `duplicate_names_and_undefined_or_unclosed_formats_are_rejected`, `configuration_bytes_and_format_depth_are_bounded`, `module_value_and_candidate_counts_are_bounded` | Record the selected root for relative, empty, and unset XDG and for a relative HOME. Also confirm that an invalid reload is not an execution revocation |
| T04 | [crates/core/src/git/tests.rs](../crates/core/src/git/tests.rs): `git_runs_the_fixed_status_argv`, `disabled_git_runs_no_git_process`, `failed_or_missing_git_shows_no_git_and_keeps_the_directory`, `repository_search_stops_after_the_depth_limit` | With a harmless marker fixture for the helper, check the external launch conditions for each Git version deployed. The tagged tests pin the argv, `git.disabled`, failure handling, and the depth bound; do not use them or the Git display tests as evidence of helper isolation |
| T05 | [crates/core/src/acquire/tests.rs](../crates/core/src/acquire/tests.rs): `file_distinguishes_missing_empty_oversized_and_non_regular_sources`; [crates/core/src/plan/tests.rs](../crates/core/src/plan/tests.rs): `source_and_regex_validation_happens_before_acquisition`, `conditions_are_relative_paths_and_bounded` | The tagged tests pin lexical path validation and the regular-file check, not containment. Record the read target for a symlink / hardlink to a dummy file outside the cwd, `gitdir:`, and substitution after the condition. Do not write an assertion that the current implementation rejects them |
| T07 / G3·G5 | [crates/cli/tests/worker_contract.rs](../crates/cli/tests/worker_contract.rs): `oversized_snapshot_is_rejected_without_running_a_command`; [crates/core/src/acquire/tests.rs](../crates/core/src/acquire/tests.rs): `command_rejects_excessive_stdout_and_releases_capacity`, `timeout_stops_descendants_that_hold_stdout_after_the_leader_exits`, `cancellation_stops_running_commands_and_prevents_queued_spawns`, `dropped_command_future_retains_its_slot_until_the_child_is_reaped`, `file_distinguishes_missing_empty_oversized_and_non_regular_sources`, `cancelled_blocking_jobs_keep_both_slots_until_their_actual_completion`; [crates/core/src/plan/tests.rs](../crates/core/src/plan/tests.rs): `configuration_bytes_and_format_depth_are_bounded`, `module_value_and_candidate_counts_are_bounded`, `environment_size_limit_applies_before_copying_and_allows_fallback`; [crates/core/src/view/tests.rs](../crates/core/src/view/tests.rs): `repeated_references_cannot_amplify_bounded_values`, `rendered_lines_fit_the_columns_and_the_response_frame`; [crates/core/src/view/text.rs](../crates/core/src/view/text.rs): `truncation_fits_and_keeps_a_grapheme_prefix` | Check cleanup within the process group and resource limits on isolated fixtures. Record group departure and non-terminating filesystems as outside the guarantee |
| T08 / G4·G5 | [crates/cli/tests/shell_contract.rs](../crates/cli/tests/shell_contract.rs): `responses_must_match_exact_generation`, `partial_response_returns_before_the_remaining_bytes_arrive`; [crates/protocol/src/session.rs](../crates/protocol/src/session.rs): `invalid_frames_fail_without_exposing_values`, `size_limits_include_the_bound`, `decoders_reject_arbitrary_frames_without_panicking`, `rejected_frames_do_not_expose_environment_values`, `framing_is_independent_of_chunking_and_cancellation`; [scripts/pty_suite/scenarios/lifecycle.py](../scripts/pty_suite/scenarios/lifecycle.py): `run_functional`, `run_cleanup`, `run_parallel` | Check backpressure, worker failure, exit / exec, and multiple shells in a real PTY, recording macOS / Linux results separately |
| T09 / G4 | [crates/cli/tests/worker_contract.rs](../crates/cli/tests/worker_contract.rs): `same_snapshot_retains_all_pending_frames_and_commits_external_changes_together`, `cancellation_and_failed_reload_retain_the_last_settled_generation`, `changed_environment_cwd_and_valid_config_clear_retained_information` | Check retention during revalidation and removal of failures and omissions at completion. Do not cite as evidence of continuous filesystem monitoring or atomic reads |
| T10 / G6 | [scripts/release/check_test.sh](../scripts/release/check_test.sh) | Follow [releasing.md](releasing.md). Separate the local contract test from the administrator's check of external publishing settings. Confirm rejection of an existing tag at a different SHA and the identity of candidate, archive, and attestation. Obtain separate approval for publishing, tag pushes, and external permission changes |

Update this document whenever any of the following changes: the CLI / IPC public surface, adding a shared worker or socket, config discovery or symlink policy, selection of execution targets or environment, Git argv, sanitize / style / prompt expansion, retained display and generation, limits and cleanup, and the trust model of dependencies, Actions, and the publishing path. For each changed boundary, always map the actor, input, asset, impact, current mitigation, residual risk, and verification location.
