use crate::{
    config::Config,
    module::{
        CmdDurationModule, CustomModuleInfo, DirectoryModule, Module, RenderContext, TimeModule,
    },
    render::{PromptLines, compose_segments},
};

/// ASCII Record Separator — delimits key from value in `char_meta` entries.
const RS: char = '\x1e';
/// ASCII Unit Separator — delimits entries in `char_meta`.
const US: char = '\x1f';

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

/// Compute built-in fast modules and combine with pre-detected custom modules.
///
/// Custom module detection is done by the caller (potentially in parallel).
pub(super) fn run_fast_modules(
    ctx: &RenderContext<'_>,
    config: &Config,
    read_only: bool,
    custom_modules: Vec<CustomModuleInfo>,
) -> FastOutputs {
    let time = if config.time.disabled {
        None
    } else {
        TimeModule::with_show_seconds(config.time.show_seconds())
            .render(ctx)
            .map(|output| output.content)
    };
    FastOutputs {
        directory: if config.directory.disabled {
            None
        } else {
            DirectoryModule::new()
                .render(ctx)
                .map(|output| output.content)
        },
        cmd_duration: if config.cmd_duration.disabled {
            None
        } else {
            CmdDurationModule::with_threshold(config.cmd_duration.threshold_ms)
                .render(ctx)
                .map(|output| output.content)
        },
        time,
        character: if config.character.disabled {
            None
        } else {
            Some(config.character.glyph.clone())
        },
        last_exit_code: ctx.last_exit_code,
        read_only,
        custom_modules,
    }
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
    let connector_style = config.connectors.prompt_style();

    let mut line1 = Vec::with_capacity(4);

    if let Some(dir) = &fast.directory {
        line1.push(
            config
                .directory
                .to_segment(dir, fast.read_only, config.color_map),
        );
    }

    if let Some(git) = slow.and_then(|output| output.git.as_deref()) {
        line1.push(config.git.to_segment(git, connector_style));
    }

    for module in &fast.custom_modules {
        line1.push(module.to_segment(connector_style));
    }
    if let Some(custom_modules) = slow.map(|output| &output.custom_modules) {
        for module in custom_modules {
            line1.push(module.to_segment(connector_style));
        }
    }

    if let Some(duration) = &fast.cmd_duration {
        line1.push(config.cmd_duration.to_segment(duration, connector_style));
    }

    let mut line2 = Vec::with_capacity(2);

    if let Some(time) = &fast.time {
        line2.push(config.time.to_segment(time, connector_style));
    }

    let viins_seg = fast
        .character
        .as_deref()
        .map(|glyph| config.character.to_segment(glyph, fast.last_exit_code));
    if let Some(ref seg) = viins_seg {
        line2.push(seg.clone());
    }

    let mut result = compose_segments(&line1, &line2, cols, config.color_map);

    if let Some(viins) = &viins_seg {
        let vicmd_seg = config
            .character
            .mode_segment(&config.character.vicmd, fast.last_exit_code);
        let viins_styled = viins.render(config.color_map);
        let vicmd_styled = vicmd_seg.render(config.color_map);
        result.char_meta = format!("viins{RS}{viins_styled}{US}vicmd{RS}{vicmd_styled}");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        module::preset_module_defs,
        render::style::{Color, Style},
        test_utils::contains_style_sequence,
    };

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
        let presets = preset_module_defs();
        let preset = presets.iter().find(|def| def.name == name);
        let style = preset.map_or(Style::new().fg(Color::BrightBlack), |def| {
            def.style
                .resolve(Style::new().fg(Color::BrightBlack).bold())
        });
        CustomModuleInfo {
            name: name.to_owned(),
            value: version.to_owned(),
            icon: preset.and_then(|def| def.icon.clone()),
            style,
            connector: Some("via".to_owned()),
        }
    }

    fn contains_yellow_ansi(line: &str) -> bool {
        line.contains("\x1b[33m")
            || contains_style_sequence(line, &[1, 33])
            || contains_style_sequence(line, &[33, 1])
    }

    #[test]
    fn test_fast_only() {
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
    fn test_with_slow() {
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
    fn test_none_git() {
        let fast = make_fast_outputs();
        let slow = make_slow_output();
        let without_slow = compose_prompt(&fast, None, 80, &default_config());
        let with_none_git = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert_eq!(without_slow, with_none_git);
    }

    #[test]
    fn test_directory_style() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            contains_style_sequence(&lines.left1, &[1, 36]),
            "directory should be bold cyan: {}",
            lines.left1
        );
    }

    #[test]
    fn test_character_success_style() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains("\x1b[32m"),
            "character should be green on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_character_error_style() {
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
    fn test_toolchain_style() {
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
        assert!(
            contains_style_sequence(&lines.left1, &[1, 31]),
            "rust toolchain should use bold red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_toolchain_omitted_without_slow() {
        let fast = make_fast_outputs();
        let without_slow = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !without_slow.left1.contains("via"),
            "toolchain should not appear without slow output: {}",
            without_slow.left1
        );

        let empty_slow = SlowOutput {
            custom_modules: vec![],
            ..make_slow_output()
        };
        let with_empty_slow = compose_prompt(&fast, Some(&empty_slow), 80, &default_config());
        assert!(
            !with_empty_slow.left1.contains("via"),
            "toolchain should not appear with empty slow output: {}",
            with_empty_slow.left1
        );
    }

    #[test]
    fn test_time_on_line2() {
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
    fn test_connector_styles() {
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
            contains_yellow_ansi(&lines.left2),
            "time content should use yellow styling: {}",
            lines.left2
        );
    }

    #[test]
    fn test_branch_icon() {
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
    fn test_cmd_duration_connector() {
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
    fn test_readonly_lock_style() {
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
    fn test_writable_no_lock_icon() {
        let lines = compose_prompt(&make_fast_outputs(), None, 80, &default_config());
        assert!(
            !lines.left1.contains('\u{f023}'),
            "writable dir should not show lock icon: {}",
            lines.left1
        );
    }

    #[test]
    fn test_custom_character_glyph() {
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
    fn test_custom_character_colors() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.character.success_style.fg = Some(Color::Magenta);
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            lines.left2.contains("\x1b[35m"),
            "character should use magenta on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_custom_directory_color() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.directory.style.fg = Some(Color::Green);
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            contains_style_sequence(&lines.left1, &[1, 32]),
            "directory should use bold green: {}",
            lines.left1
        );
    }

    #[test]
    fn test_custom_connectors() {
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
        config.git.connector = "branch".to_owned();
        config.time.connector = "time".to_owned();
        config.cmd_duration.connector = "duration".to_owned();
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
    fn test_time_disabled() {
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
    fn test_custom_git_icon() {
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
    fn test_custom_cmd_duration_color() {
        let fast = FastOutputs {
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let mut config = default_config();
        config.cmd_duration.style.fg = Some(Color::Red);
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            contains_style_sequence(&lines.left1, &[1, 31])
                || contains_style_sequence(&lines.left1, &[31, 1]),
            "cmd_duration should use bold red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_structured_styles_and_color_map() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            cmd_duration: Some("3s".to_owned()),
            read_only: true,
            ..make_fast_outputs()
        };
        let slow = SlowOutput {
            git: Some("main [!+]".to_owned()),
            custom_modules: vec![make_toolchain_module("rust", "v1.82.0")],
        };
        let mut config = default_config();
        config.directory.style.fg = Some(Color::Blue);
        config.directory.style.bold = Some(false);
        config.directory.read_only_style.fg = Some(Color::Yellow);
        config.directory.read_only_style.bold = Some(true);
        config.git.style.fg = Some(Color::Cyan);
        config.git.style.bold = Some(false);
        config.git.indicator_style.fg = Some(Color::Yellow);
        config.git.indicator_style.bold = Some(false);
        config.time.style.fg = Some(Color::Blue);
        config.time.style.dimmed = Some(true);
        config.cmd_duration.style.fg = Some(Color::Yellow);
        config.cmd_duration.style.bold = Some(true);
        config.character.success_style.fg = Some(Color::Magenta);
        config.character.success_style.bold = Some(true);
        config.connectors.style.fg = Some(Color::BrightBlack);
        config.connectors.style.dimmed = Some(true);
        config.color_map.blue = 94;
        config.color_map.yellow = 93;
        config.color_map.magenta = 95;
        config.color_map.cyan = 96;
        config.color_map.bright_black = 37;

        let lines = compose_prompt(&fast, Some(&slow), 120, &config);

        assert!(
            lines.left1.contains("\x1b[94m"),
            "directory should use remapped blue: {}",
            lines.left1
        );
        assert!(
            contains_style_sequence(&lines.left1, &[1, 93])
                || contains_style_sequence(&lines.left1, &[93, 1]),
            "read-only lock should use bold remapped yellow: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("\x1b[96m"),
            "git branch/icon should use remapped cyan: {}",
            lines.left1
        );
        assert!(
            contains_style_sequence(&lines.left1, &[37, 2])
                || contains_style_sequence(&lines.left1, &[2, 37]),
            "connectors should use configured dimmed bright_black mapping: {}",
            lines.left1
        );
        assert!(
            contains_style_sequence(&lines.left2, &[2, 94])
                || contains_style_sequence(&lines.left2, &[94, 2]),
            "time should use dimmed remapped blue: {}",
            lines.left2
        );
        assert!(
            contains_style_sequence(&lines.left2, &[1, 95])
                || contains_style_sequence(&lines.left2, &[95, 1]),
            "character should use bold remapped magenta: {}",
            lines.left2
        );
    }

    #[test]
    fn test_char_meta_empty_when_disabled() {
        let fast = FastOutputs {
            character: None,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.char_meta.is_empty(),
            "char_meta should be empty when character is disabled"
        );
    }

    #[test]
    fn test_char_meta_with_default_config() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.char_meta.is_empty(),
            "char_meta should be populated by default"
        );
        assert!(
            lines.char_meta.contains("viins\x1e"),
            "char_meta should contain viins entry"
        );
        assert!(
            lines.char_meta.contains("\x1fvicmd\x1e"),
            "char_meta should contain vicmd entry"
        );
    }

    #[test]
    fn test_char_meta_custom_vicmd_style() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.character.vicmd = crate::config::CharacterModeConfig {
            glyph: "❮".to_owned(),
            style: Some(crate::config::StyleConfig::fg(Color::Green)),
        };
        let lines = compose_prompt(&fast, None, 80, &config);
        assert!(
            !lines.char_meta.is_empty(),
            "char_meta should be populated when vicmd has custom style"
        );
    }

    #[test]
    fn test_char_meta_viins_matches_left2() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        // Extract viins styled string from char_meta
        let viins_entry = lines
            .char_meta
            .split('\x1f')
            .find(|e| e.starts_with("viins\x1e"));
        let viins_styled = viins_entry.map_or("", |e| &e["viins\x1e".len()..]);
        assert!(
            !viins_styled.is_empty(),
            "viins styled string should not be empty"
        );
        assert!(
            lines.left2.contains(viins_styled),
            "viins styled string should appear in left2: left2={}, viins={}",
            lines.left2,
            viins_styled
        );
    }
}
