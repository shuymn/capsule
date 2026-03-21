use std::path::Path;

use crate::{
    config::Config,
    module::{
        CmdDurationModule, CustomModuleInfo, DirectoryModule, GitModule, GitProvider, Module,
        ModuleSpeed, RenderContext, ResolvedModule, TimeModule, detect_modules,
    },
    render::{
        PromptLines, compose_segments,
        segment::{Connector, Icon, Segment},
        style::{Color, Style},
    },
};

#[derive(Debug, Clone)]
pub(super) struct FastOutputs {
    directory: Option<String>,
    cmd_duration: Option<String>,
    time: Option<String>,
    character: Option<String>,
    last_exit_code: i32,
    read_only: bool,
    custom_modules: Vec<CustomModuleInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SlowOutput {
    pub(super) git: Option<String>,
    pub(super) custom_modules: Vec<CustomModuleInfo>,
}

pub(super) struct SlowModulesInput<'a, G> {
    pub(super) cwd: &'a Path,
    pub(super) provider: G,
    pub(super) indicator_color: Color,
    pub(super) path_env: Option<&'a str>,
    pub(super) modules: &'a [ResolvedModule],
    pub(super) env_vars: &'a [(String, String)],
}

pub(super) fn run_fast_modules(
    ctx: &RenderContext<'_>,
    config: &Config,
    modules: &[ResolvedModule],
    env_vars: &[(String, String)],
) -> FastOutputs {
    let read_only = std::fs::metadata(ctx.cwd).is_ok_and(|m| m.permissions().readonly());
    let time = if config.time.enabled {
        TimeModule::with_show_seconds(config.time.show_seconds())
            .render(ctx)
            .map(|output| output.content)
    } else {
        None
    };
    let custom_modules = detect_modules(modules, ctx.cwd, env_vars, None, ModuleSpeed::Fast);
    FastOutputs {
        directory: DirectoryModule::new()
            .render(ctx)
            .map(|output| output.content),
        cmd_duration: CmdDurationModule::with_threshold(config.cmd_duration.threshold_ms)
            .render(ctx)
            .map(|output| output.content),
        time,
        character: Some(config.character.glyph.clone()),
        last_exit_code: ctx.last_exit_code,
        read_only,
        custom_modules,
    }
}

pub(super) fn run_slow_modules<G: GitProvider>(input: SlowModulesInput<'_, G>) -> SlowOutput {
    let git_module = GitModule::with_indicator_color(input.provider, input.indicator_color);
    let custom_modules = detect_modules(
        input.modules,
        input.cwd,
        input.env_vars,
        input.path_env,
        ModuleSpeed::Slow,
    );
    SlowOutput {
        git: git_module
            .render_for_cwd(input.cwd, input.path_env)
            .map(|output| output.content),
        custom_modules,
    }
}

const CONNECTOR_STYLE: Style = Style::new();

fn make_connector(word: &str) -> Connector {
    Connector {
        word: word.to_owned(),
        style: CONNECTOR_STYLE,
    }
}

fn make_icon(glyph: &str, style: Style) -> Icon {
    Icon {
        glyph: glyph.to_owned(),
        style,
    }
}

fn push_custom_module_segment(segments: &mut Vec<Segment>, module: &CustomModuleInfo) {
    let connector = module.connector.as_deref().map(make_connector);
    let icon = module
        .icon
        .as_deref()
        .map(|glyph| make_icon(glyph, module.style));
    segments.push(Segment {
        content: module.value.clone(),
        connector,
        icon,
        content_style: Some(module.style),
    });
}

/// Prompt layout (Starship-compatible):
/// - Info line (left1):  `[directory] on [git] via [toolchain] [cmd_duration]`
/// - Input line (left2): `at [time] [character]`
pub(super) fn compose_prompt(
    fast: &FastOutputs,
    slow: Option<&SlowOutput>,
    cols: usize,
    config: &Config,
) -> PromptLines {
    let dir_style = Style::new().fg(config.directory.color).bold();

    let mut line1 = Vec::with_capacity(4);

    if let Some(dir) = &fast.directory {
        if fast.read_only {
            let lock_style = Style::new().fg(Color::Red);
            let content = format!("{} {}", dir_style.paint(dir), lock_style.paint("\u{f023}"));
            line1.push(Segment {
                content,
                connector: None,
                icon: None,
                content_style: None,
            });
        } else {
            line1.push(Segment {
                content: dir.clone(),
                connector: None,
                icon: None,
                content_style: Some(dir_style),
            });
        }
    }

    if let Some(git) = slow.and_then(|output| output.git.as_deref()) {
        line1.push(Segment {
            content: git.to_owned(),
            connector: Some(make_connector(&config.connectors.git)),
            icon: Some(make_icon(&config.git.icon, Style::new().fg(Color::Magenta))),
            content_style: None,
        });
    }

    for module in &fast.custom_modules {
        push_custom_module_segment(&mut line1, module);
    }
    if let Some(custom_modules) = slow.map(|output| &output.custom_modules) {
        for module in custom_modules {
            push_custom_module_segment(&mut line1, module);
        }
    }

    if let Some(duration) = &fast.cmd_duration {
        line1.push(Segment {
            content: duration.clone(),
            connector: Some(make_connector(&config.connectors.cmd_duration)),
            icon: None,
            content_style: Some(Style::new().fg(config.cmd_duration.color)),
        });
    }

    let mut line2 = Vec::with_capacity(2);

    if let Some(time) = &fast.time {
        line2.push(Segment {
            content: time.clone(),
            connector: Some(make_connector(&config.connectors.time)),
            icon: None,
            content_style: Some(Style::new().fg(config.time.color)),
        });
    }

    if let Some(character) = &fast.character {
        let char_style = if fast.last_exit_code == 0 {
            Style::new().fg(config.character.success_color)
        } else {
            Style::new().fg(config.character.error_color)
        };
        line2.push(Segment {
            content: character.clone(),
            connector: None,
            icon: None,
            content_style: Some(char_style),
        });
    }

    compose_segments(&line1, &line2, cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, module::resolve_modules};

    fn default_config() -> Config {
        Config::default()
    }

    fn make_fast_outputs() -> FastOutputs {
        FastOutputs {
            directory: Some("/tmp".to_owned()),
            cmd_duration: None,
            time: None,
            character: Some("\u{276f}".to_owned()),
            last_exit_code: 0,
            read_only: false,
            custom_modules: vec![],
        }
    }

    fn make_slow_output() -> SlowOutput {
        SlowOutput {
            git: None,
            custom_modules: vec![],
        }
    }

    fn make_toolchain_module(name: &str, version: &str) -> CustomModuleInfo {
        let defs = resolve_modules(&[]);
        let resolved = defs.iter().find(|def| def.name == name);
        CustomModuleInfo {
            name: name.to_owned(),
            value: version.to_owned(),
            icon: resolved.and_then(|def| def.icon.clone()),
            style: resolved.map_or(Style::new().fg(Color::BrightBlack), |def| def.style),
            connector: Some("via".to_owned()),
        }
    }

    fn contains_yellow_ansi(line: &str) -> bool {
        line.contains("\x1b[33m")
    }

    #[test]
    fn test_daemon_compose_prompt_fast_only() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        assert!(
            lines.left2.contains("at"),
            "left2 should have 'at': {}",
            lines.left2
        );
        assert!(
            lines.left2.contains("14:30:45"),
            "left2 should have time: {}",
            lines.left2
        );
        assert!(
            lines.left2.contains('\u{276f}'),
            "left2 should have character: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_with_slow() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        assert!(
            lines.left1.contains("on"),
            "left1 should contain 'on' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("main"),
            "left1 should contain branch: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_slow_none_git() {
        let fast = make_fast_outputs();
        let slow = make_slow_output();
        let without_slow = compose_prompt(&fast, None, 80, &default_config());
        let with_none_git = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert_eq!(without_slow, with_none_git);
    }

    #[test]
    fn test_daemon_compose_prompt_styled_directory() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains("\x1b[1;36m"),
            "directory should be bold cyan: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_styled_character_success() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains("\x1b[32m"),
            "character should be green on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_styled_character_error() {
        let fast = FastOutputs {
            last_exit_code: 1,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains("\x1b[31m"),
            "character should be red on error: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_with_toolchain_version() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            custom_modules: vec![make_toolchain_module("rust", "v1.82.0")],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("via"),
            "left1 should contain 'via' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("v1.82.0"),
            "left1 should contain version: {}",
            lines.left1
        );
        assert!(
            !lines.left1.contains("rust"),
            "left1 should not contain toolchain name: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_toolchain_uses_theme_color() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            custom_modules: vec![make_toolchain_module("rust", "v1.82.0")],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("\x1b[1;31m"),
            "rust toolchain should use bold red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_no_toolchain_without_slow() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left1.contains("via"),
            "toolchain should not appear without slow output: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_multiple_toolchains() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            custom_modules: vec![
                make_toolchain_module("rust", "v1.82.0"),
                make_toolchain_module("node", "v22.0.0"),
            ],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 120, &default_config());
        assert!(
            lines.left1.contains("v1.82.0"),
            "should contain rust version: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("v22.0.0"),
            "should contain node version: {}",
            lines.left1
        );
        assert_eq!(
            lines.left1.matches("via").count(),
            2,
            "should have two 'via' connectors: {}",
            lines.left1
        );
        let rust_pos = lines.left1.find("v1.82.0");
        let node_pos = lines.left1.find("v22.0.0");
        assert!(
            rust_pos < node_pos,
            "rust should come before node: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_empty_custom_modules() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            custom_modules: vec![],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            !lines.left1.contains("via"),
            "no 'via' connector with empty custom modules: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_time_on_line2() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left1.contains("14:30:45"),
            "time should not be on line 1: {}",
            lines.left1
        );
        assert!(
            lines.left2.contains("14:30:45"),
            "time should be on line 2: {}",
            lines.left2
        );
        assert!(
            contains_yellow_ansi(&lines.left2),
            "time should use yellow styling: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_does_not_dim_connectors() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            custom_modules: vec![make_toolchain_module("rust", "v1.82.0")],
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("on"),
            "git connector should be present: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("via"),
            "toolchain connector should be present: {}",
            lines.left1
        );
        assert!(
            lines.left2.contains("at"),
            "time connector should be present: {}",
            lines.left2
        );
        assert!(
            !lines.left1.contains("\x1b[90mon\x1b[0m")
                && !lines.left1.contains("\x1b[90mvia\x1b[0m"),
            "connectors should not use bright black: {}",
            lines.left1
        );
        assert!(
            !lines.left2.contains("\x1b[90mat\x1b[0m"),
            "time connector should not use bright black: {}",
            lines.left2
        );
        assert!(
            lines.left1.contains("\x1b[1;31m"),
            "rust toolchain should use bold red: {}",
            lines.left1
        );
        assert!(
            contains_yellow_ansi(&lines.left2),
            "time content should use yellow styling: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_branch_icon_f418() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains('\u{f418}'),
            "branch icon should be \\u{{f418}}: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_cmd_duration_took_connector() {
        let fast = FastOutputs {
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains("took"),
            "cmd_duration should have 'took' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("3s"),
            "cmd_duration should contain duration: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_readonly_shows_lock_icon() {
        let fast = FastOutputs {
            read_only: true,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains('\u{f023}'),
            "readonly dir should show lock icon: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_readonly_lock_styled_red() {
        let fast = FastOutputs {
            read_only: true,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        let lock_pos = lines.left1.find('\u{f023}');
        assert!(lock_pos.is_some(), "lock icon should be present");
        let before_lock = &lines.left1[..lock_pos.unwrap_or(0)];
        assert!(
            before_lock.contains("\x1b[31m"),
            "lock icon should be styled red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_writable_no_lock_icon() {
        let lines = compose_prompt(&make_fast_outputs(), None, 80, &default_config());
        assert!(
            !lines.left1.contains('\u{f023}'),
            "writable dir should not show lock icon: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_character_glyph() {
        let fast = FastOutputs {
            character: Some("$".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains('$'),
            "left2 should contain custom glyph '$': {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_character_colors() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.character.success_color = Color::Magenta;
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            lines.left2.contains("\x1b[35m"),
            "character should use magenta on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_directory_color() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.directory.color = Color::Green;
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            lines.left1.contains("\x1b[1;32m"),
            "directory should use bold green: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_connectors() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let mut config = default_config();
        config.connectors.git = "branch".to_owned();
        config.connectors.time = "time".to_owned();
        config.connectors.cmd_duration = "duration".to_owned();
        let lines = compose_prompt(&fast, Some(&slow), 80, &config);
        assert!(
            lines.left1.contains("branch"),
            "git connector should be 'branch': {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("duration"),
            "cmd_duration connector should be 'duration': {}",
            lines.left1
        );
        assert!(
            lines.left2.contains("time"),
            "time connector should be 'time': {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_time_disabled() {
        let fast = FastOutputs {
            time: None,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left2.contains("at"),
            "time connector should not appear when time is None: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_git_icon() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let mut config = default_config();
        config.git.icon = "\u{e0a0}".to_owned();
        let lines = compose_prompt(&fast, Some(&slow), 80, &config);
        assert!(
            lines.left1.contains('\u{e0a0}'),
            "git icon should be custom icon: {}",
            lines.left1
        );
        assert!(
            !lines.left1.contains('\u{f418}'),
            "default git icon should not appear: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_cmd_duration_color() {
        let fast = FastOutputs {
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let mut config = default_config();
        config.cmd_duration.color = Color::Red;
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            lines.left1.contains("\x1b[31m"),
            "cmd_duration should use red: {}",
            lines.left1
        );
    }
}
