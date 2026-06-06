use super::*;

#[test]
fn load_missing_file_returns_defaults() {
    let config = load_config(Path::new("/nonexistent/config.toml"));
    assert_eq!(config.character.glyph, "\u{276f}");
    assert_eq!(config.cmd_duration.threshold_ms, 2000);
}

#[test]
fn load_empty_file_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "")?;
    let config = load_config(&path);
    assert_eq!(config.character.glyph, "\u{276f}");
    assert_eq!(config.cmd_duration.threshold_ms, 2000);
    Ok(())
}

#[test]
fn load_partial_overrides() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character]
glyph = "$"

[cmd_duration]
threshold_ms = 5000
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.glyph, "$");
    assert_eq!(config.cmd_duration.threshold_ms, 5000);
    // Non-overridden fields keep defaults
    assert_eq!(config.character.success_style.fg, Some(Color::Green));
    assert_eq!(config.directory.style.fg, Some(Color::Cyan));
    Ok(())
}

#[test]
fn load_syntax_error_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "invalid = [toml content")?;
    let config = load_config(&path);
    assert_eq!(config.character.glyph, "\u{276f}");
    Ok(())
}

#[test]
fn time_disabled() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r"
[time]
disabled = true
",
    )?;
    let config = load_config(&path);
    assert!(config.time.disabled);
    Ok(())
}

#[test]
fn style_fg_deserializes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.success_style]
fg = "magenta"

[character.error_style]
fg = "yellow"

[directory.style]
fg = "blue"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.success_style.fg, Some(Color::Magenta));
    assert_eq!(config.character.error_style.fg, Some(Color::Yellow));
    assert_eq!(config.directory.style.fg, Some(Color::Blue));
    Ok(())
}

#[test]
fn connector_overrides() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[git]
connector = "branch"

[time]
connector = "time"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.git.connector, "branch");
    assert_eq!(config.time.connector, "time");
    // Non-overridden connectors keep defaults
    assert_eq!(config.cmd_duration.connector, "took");
    Ok(())
}

#[test]
fn time_format_show_seconds() {
    let mut config = TimeConfig::default();
    assert!(config.show_seconds());

    config.format = TimeFormat::WithoutSeconds;
    assert!(!config.show_seconds());

    config.format = TimeFormat::WithSeconds;
    assert!(config.show_seconds());
}

#[test]
fn time_format_deserializes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[time]
format = "HH:MM"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.time.format, TimeFormat::WithoutSeconds);
    Ok(())
}

#[test]
fn time_format_invalid_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[time]
format = "INVALID"
"#,
    )?;
    // Invalid format causes parse error → defaults
    let config = load_config(&path);
    assert_eq!(config.time.format, TimeFormat::WithSeconds);
    Ok(())
}

#[test]
fn regex_pattern_invalid_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "bad"

[[module.source]]
command = ["echo", "x"]
regex = "(unclosed"
"#,
    )?;
    // Invalid regex causes parse error → defaults
    let config = load_config(&path);
    assert!(config.module.is_empty());
    Ok(())
}

#[test]
fn git_disabled() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r"
[git]
disabled = true
",
    )?;
    let config = load_config(&path);
    assert!(config.git.disabled);
    Ok(())
}

#[test]
fn git_enabled_by_default() {
    let config = Config::default();
    assert!(!config.git.disabled);
}

#[test]
fn git_icon_override() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[git]
icon = ""

[git.indicator_style]
fg = "yellow"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.git.icon, "");
    assert_eq!(config.git.indicator_style.fg, Some(Color::Yellow));
    Ok(())
}

#[test]
fn style_overrides_and_color_map() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[directory.style]
fg = "blue"
bold = false
dimmed = true

[directory.read_only_style]
fg = "yellow"
bold = true

[connectors.style]
fg = "bright_black"
dimmed = true

[color_map]
blue = 94
bright_black = 37
"#,
    )?;
    let config = read_config(&path)?.ok_or("config missing")?;
    assert_eq!(config.directory.style.fg, Some(Color::Blue));
    assert_eq!(config.directory.style.bold, Some(false));
    assert_eq!(config.directory.style.dimmed, Some(true));
    assert_eq!(config.directory.read_only_style.fg, Some(Color::Yellow));
    assert_eq!(config.directory.read_only_style.bold, Some(true));
    assert_eq!(config.connectors.style.fg, Some(Color::BrightBlack));
    assert_eq!(config.connectors.style.dimmed, Some(true));
    assert_eq!(config.color_map.blue, 94);
    assert_eq!(config.color_map.bright_black, 37);
    Ok(())
}

#[test]
fn invalid_color_map_code_fails_loading() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r"
[color_map]
red = 38
",
    )?;
    let result = read_config(&path);
    assert!(matches!(result, Err(ConfigLoadError::Parse { .. })));
    Ok(())
}

// -- [[module]] config tests -----------------------------------------------

#[test]
fn module_empty_by_default() {
    let config = Config::default();
    assert!(config.module.is_empty());
}

#[test]
fn module_parse_env_source() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "aws"
when.env = ["AWS_PROFILE"]
format = "{value}"

[[module.source]]
env = "AWS_PROFILE"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.module.len(), 1);
    assert_eq!(config.module[0].name, "aws");
    assert_eq!(config.module[0].when.env, ["AWS_PROFILE"]);
    assert_eq!(config.module[0].format, "{value}");
    assert_eq!(config.module[0].source.len(), 1);
    assert_eq!(config.module[0].arbitration, None);
    assert_eq!(
        config.module[0].source[0].env.as_deref(),
        Some("AWS_PROFILE")
    );
    assert!(!config.module[0].source[0].is_command());
    Ok(())
}

#[test]
fn module_parse_command_source_with_regex() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "zig"
when.files = ["build.zig"]
format = "v{value}"
icon = "Z"
connector = "via"

[module.style]
fg = "yellow"

[[module.source]]
command = ["zig", "version"]
regex = '(\d[\d.]*)'
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.module.len(), 1);
    let m = &config.module[0];
    assert_eq!(m.name, "zig");
    assert_eq!(m.when.files, ["build.zig"]);
    assert_eq!(m.format, "v{value}");
    assert_eq!(m.icon.as_deref(), Some("Z"));
    assert_eq!(m.style.fg, Some(Color::Yellow));
    assert_eq!(m.connector.as_deref(), Some("via"));
    assert_eq!(m.source.len(), 1);
    assert!(m.source[0].is_command());
    assert!(m.source[0].regex.is_some());
    Ok(())
}

#[test]
fn module_default_format() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "test"

[[module.source]]
env = "FOO"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.module[0].format, "{value}");
    Ok(())
}

#[test]
fn module_multiple_sources() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "node"
when.files = ["package.json"]
format = "v{value}"
connector = "via"

[[module.source]]
file = ".node-version"

[[module.source]]
command = ["node", "--version"]
regex = 'v?(\d[\d.]*)'
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.module[0].source.len(), 2);
    assert!(config.module[0].source[0].file.is_some());
    assert!(config.module[0].source[1].command.is_some());
    Ok(())
}

#[test]
fn module_parse_arbitration() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "node"

[module.arbitration]
group = "node.js"
priority = 20

[[module.source]]
env = "NODE_VERSION"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(
        config.module[0].arbitration,
        Some(Arbitration {
            group: "node.js".to_owned(),
            priority: 20,
        })
    );
    Ok(())
}

#[test]
fn module_source_requires_exactly_one_source_kind() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let missing_path = dir.path().join("missing.toml");
    std::fs::write(
        &missing_path,
        r#"
[[module]]
name = "missing"

[[module.source]]
name = "value"
"#,
    )?;
    assert!(matches!(
        read_config(&missing_path),
        Err(ConfigLoadError::Parse { .. })
    ));

    let duplicate_path = dir.path().join("duplicate.toml");
    std::fs::write(
        &duplicate_path,
        r#"
[[module]]
name = "duplicate"

[[module.source]]
env = "PROFILE"
command = ["echo", "fallback"]
"#,
    )?;
    assert!(matches!(
        read_config(&duplicate_path),
        Err(ConfigLoadError::Parse { .. })
    ));
    Ok(())
}

#[test]
fn module_source_rejects_empty_command() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "empty-command"

[[module.source]]
command = []
"#,
    )?;

    assert!(matches!(
        read_config(&path),
        Err(ConfigLoadError::Parse { .. })
    ));
    Ok(())
}

#[test]
fn module_source_rejects_unknown_fields() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "unknown-source-field"

[[module.source]]
env = "PROFILE"
unexpected = "value"
"#,
    )?;

    assert!(matches!(
        read_config(&path),
        Err(ConfigLoadError::Parse { .. })
    ));
    Ok(())
}

#[test]
fn module_source_rejects_unsafe_file_path() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    for (case_name, file_path) in [
        ("parent", "../secret"),
        ("absolute", "/etc/passwd"),
        ("current", "./version"),
    ] {
        let path = dir.path().join(format!("{case_name}.toml"));
        std::fs::write(
            &path,
            format!(
                r#"
[[module]]
name = "{case_name}"

[[module.source]]
file = "{file_path}"
"#
            ),
        )?;
        assert!(
            matches!(read_config(&path), Err(ConfigLoadError::Parse { .. })),
            "{file_path} should be rejected"
        );
    }
    Ok(())
}

#[test]
fn empty_strings_preserve_empty() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character]
glyph = ""
[git]
icon = ""
connector = ""
[time]
connector = ""
[cmd_duration]
connector = ""
"#,
    )?;
    let result = read_config(&path)?;
    let config = result.ok_or("config should parse")?;
    // Empty strings are valid — they should be preserved, not replaced with defaults
    assert_eq!(config.character.glyph, "");
    assert_eq!(config.git.icon, "");
    assert_eq!(config.git.connector, "");
    assert_eq!(config.time.connector, "");
    assert_eq!(config.cmd_duration.connector, "");
    Ok(())
}

#[test]
fn cache_defaults_to_revalidate() {
    let config = Config::default();
    assert_eq!(config.cache.slow, SlowCacheMode::Revalidate);
}

#[test]
fn cache_slow_off() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[cache]
slow = "off"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.cache.slow, SlowCacheMode::Off);
    Ok(())
}

#[test]
fn cache_slow_revalidate() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[cache]
slow = "revalidate"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.cache.slow, SlowCacheMode::Revalidate);
    Ok(())
}

#[test]
fn vicmd_default() {
    let config = Config::default();
    assert_eq!(
        config.character.vicmd.glyph, "\u{276e}",
        "default vicmd glyph should be ❮"
    );
    assert!(
        config.character.vicmd.style.is_none(),
        "default vicmd style should fall back to parent"
    );
}

#[test]
fn vicmd_deserializes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.vicmd]
glyph = "❮"
style = { fg = "green" }
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.vicmd.glyph, "❮");
    assert_eq!(
        config
            .character
            .vicmd
            .style
            .as_ref()
            .ok_or("style should be Some")?
            .fg,
        Some(Color::Green)
    );
    Ok(())
}

#[test]
fn vicmd_glyph_only() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.vicmd]
glyph = "N"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.vicmd.glyph, "N");
    assert!(
        config.character.vicmd.style.is_none(),
        "style should fall back to parent"
    );
    Ok(())
}

#[test]
fn mode_segment_with_fixed_style() {
    let config = CharacterConfig::default();
    let mode = CharacterModeConfig {
        glyph: "❮".to_owned(),
        style: Some(StyleConfig::fg(Color::Magenta)),
    };
    let seg_ok = config.mode_segment(&mode, 0);
    let seg_err = config.mode_segment(&mode, 1);
    assert_eq!(seg_ok.content, "❮");
    assert_eq!(seg_ok.content_style, seg_err.content_style);
}

#[test]
fn mode_segment_falls_back_to_parent_style() {
    let config = CharacterConfig::default();
    let mode = CharacterModeConfig {
        glyph: "❮".to_owned(),
        style: None,
    };
    let seg_ok = config.mode_segment(&mode, 0);
    let seg_err = config.mode_segment(&mode, 1);
    assert_ne!(
        seg_ok.content_style, seg_err.content_style,
        "fallback should use parent success/error styles"
    );
    assert_eq!(
        seg_ok.content_style,
        config.to_segment("x", 0).content_style
    );
    assert_eq!(
        seg_err.content_style,
        config.to_segment("x", 1).content_style
    );
}

#[test]
fn vicmd_default_glyph_preserved() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.vicmd]
style = { fg = "green" }
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(
        config.character.vicmd.glyph, "\u{276e}",
        "default vicmd glyph should be ❮ when only style is specified"
    );
    Ok(())
}

#[test]
fn partial_style_preserves_parent_bold() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.success_style]
fg = "magenta"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.success_style.fg, Some(Color::Magenta));
    assert_eq!(
        config.character.success_style.bold,
        Some(true),
        "bold from CharacterConfig default should be preserved"
    );
    Ok(())
}

#[test]
fn partial_style_explicit_false_overrides_default() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.success_style]
fg = "magenta"
bold = false
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.success_style.fg, Some(Color::Magenta));
    assert_eq!(
        config.character.success_style.bold,
        Some(false),
        "explicit bold = false should override the default true"
    );
    Ok(())
}

#[test]
fn git_partial_style_preserves_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[git.style]
fg = "cyan"
[git.indicator_style]
fg = "yellow"
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.git.style.fg, Some(Color::Cyan));
    assert_eq!(
        config.git.style.bold,
        Some(true),
        "bold from GitConfig default should be preserved for style"
    );
    assert_eq!(config.git.indicator_style.fg, Some(Color::Yellow));
    assert_eq!(
        config.git.indicator_style.bold,
        Some(true),
        "bold from GitConfig default should be preserved for indicator_style"
    );
    Ok(())
}

#[test]
fn full_style_override_not_affected() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[character.success_style]
fg = "blue"
bold = false
dimmed = true
"#,
    )?;
    let config = load_config(&path);
    assert_eq!(config.character.success_style.fg, Some(Color::Blue));
    assert_eq!(config.character.success_style.bold, Some(false));
    assert_eq!(config.character.success_style.dimmed, Some(true));
    Ok(())
}

#[test]
fn module_source_command_rejects_empty_argument() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[module]]
name = "empty-command"
source = [{ command = ["echo", ""] }]
"#,
    )?;

    assert!(matches!(
        read_config(&path),
        Err(ConfigLoadError::Parse { .. })
    ));
    Ok(())
}
