//! Keep documentation that restates code facts in sync with the code.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use capsule_core::limits;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> Result<String, std::io::Error> {
    fs::read_to_string(root().join(path))
}

/// A limit value with the unit its documentation row must use.
#[derive(Debug, PartialEq, Eq)]
enum Limit {
    Bytes(usize),
    Count(usize),
    Time(Duration),
}

macro_rules! registry {
    ($($kind:ident($name:ident)),* $(,)?) => {
        [$((stringify!($name), registry!(@value $kind, limits::$name))),*]
    };
    (@value Bytes, $value:expr) => { Limit::Bytes($value) };
    (@value Count, $value:expr) => { Limit::Count(usize::try_from($value)?) };
    (@value Time, $value:expr) => { Limit::Time($value) };
}

fn limit_registry() -> Result<BTreeMap<&'static str, Limit>, Box<dyn std::error::Error>> {
    Ok(BTreeMap::from(registry![
        Bytes(MAX_REQUEST),
        Bytes(MAX_RESPONSE),
        Bytes(MAX_SNAPSHOT_BYTES),
        Bytes(MAX_OUTPUT_BYTES),
        Bytes(MAX_CONFIG_BYTES),
        Count(MAX_PROCESSES),
        Count(MAX_FILE_JOBS),
        Time(ACQUISITION_TIMEOUT),
        Time(GENERATION_TIMEOUT),
        Time(CLEANUP_TIMEOUT),
        Count(MAX_MODULES),
        Count(MAX_TOTAL_VALUES),
        Count(MAX_TOTAL_CANDIDATES),
        Count(MAX_VALUES_PER_MODULE),
        Count(MAX_CANDIDATES_PER_VALUE),
        Count(MAX_WHEN_ITEMS),
        Count(MAX_COMMAND_ARGS),
        Bytes(MAX_FORMAT_BYTES),
        Count(MAX_FORMAT_DEPTH),
        Bytes(MAX_REGEX_BYTES),
        Count(MAX_REGEX_NESTING),
        Bytes(MAX_MODULE_TEXT_BYTES),
        Bytes(MAX_LINE_TEXT_BYTES),
        Count(MAX_REPOSITORY_DEPTH),
    ]))
}

/// Names that `limits.rs` defines or re-exports publicly.
fn public_limit_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in source.lines() {
        if let Some(rest) = line.strip_prefix("pub const ") {
            names.extend(rest.split(':').next().map(str::to_owned));
        } else if let Some(rest) = line.strip_prefix("pub use ")
            && let Some((_, list)) = rest.split_once('{')
        {
            names.extend(
                list.trim_end_matches("};")
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    names
}

fn parse_limit(text: &str) -> Option<Limit> {
    let (number, unit) = text.split_once(' ').unwrap_or((text, ""));
    let number = number.parse::<usize>().ok()?;
    Some(match unit {
        "" => Limit::Count(number),
        "bytes" => Limit::Bytes(number),
        "KiB" => Limit::Bytes(number * 1024),
        "ms" => Limit::Time(Duration::from_millis(u64::try_from(number).ok()?)),
        "s" => Limit::Time(Duration::from_secs(u64::try_from(number).ok()?)),
        _ => return None,
    })
}

/// Rows of the `| Boundary | Limit | Constant |` table, keyed by constant.
fn documented_limits(document: &str) -> Result<BTreeMap<String, Limit>, String> {
    let mut rows = BTreeMap::new();
    let mut in_table = false;
    for line in document.lines() {
        if line == "| Boundary | Limit | Constant |" {
            in_table = true;
            continue;
        }
        if !in_table || line.starts_with("| ---") {
            continue;
        }
        let Some(cells) = line.strip_prefix('|').and_then(|row| row.strip_suffix('|')) else {
            break;
        };
        let cells = cells.rsplitn(3, '|').map(str::trim).collect::<Vec<_>>();
        let [constant, value, _] = cells.as_slice() else {
            return Err(format!("malformed limit row: {line}"));
        };
        let constant = constant.trim_matches('`').to_owned();
        let value = parse_limit(value).ok_or_else(|| format!("unparsed limit: {line}"))?;
        if rows.insert(constant.clone(), value).is_some() {
            return Err(format!("duplicate limit row: {constant}"));
        }
    }
    Ok(rows)
}

#[test]
fn every_limit_is_registered_for_the_docs_check() -> TestResult {
    let registry = limit_registry()?;
    let registered = registry
        .keys()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        public_limit_names(&read("crates/core/src/limits.rs")?),
        registered
    );
    Ok(())
}

#[test]
fn threat_model_limits_table_matches_the_limits_module() -> TestResult {
    let documented = documented_limits(&read("docs/threat-model.md")?)?;
    let registry = limit_registry()?;
    let documented_names = documented
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let registered_names = registry.keys().copied().collect::<BTreeSet<_>>();
    assert_eq!(documented_names, registered_names);
    for (name, value) in &registry {
        assert_eq!(documented.get(*name), Some(value), "{name}");
    }
    Ok(())
}

#[test]
fn zsh_snapshot_bound_matches_the_limits_module() -> TestResult {
    let init = read("crates/core/src/init/init.zsh")?;
    let bound = format!(
        "(( ${{#_CAPSULE_SNAPSHOT}} > {} ))",
        limits::MAX_SNAPSHOT_BYTES
    );
    assert_eq!(init.matches(&bound).count(), 1, "{bound}");
    Ok(())
}

/// Markdown files written for agents and users; generated benchmark reports are data.
fn documents() -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = ["AGENTS.md", "CLAUDE.md", "README.md", "README.ja.md"]
        .iter()
        .map(|name| root().join(name))
        .collect::<Vec<_>>();
    let mut directories = vec![root().join("docs")];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Every identifier-like word in tracked sources, scripts, and configuration.
fn source_words() -> Result<BTreeSet<String>, std::io::Error> {
    const SKIPPED: [&str; 4] = [".git", "target", "docs", "assets"];
    let mut words = BTreeSet::new();
    let mut directories = vec![root()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if !SKIPPED.contains(&name) {
                    directories.push(path);
                }
            } else if !name.ends_with(".md")
                && let Ok(text) = fs::read_to_string(&path)
            {
                words.extend(
                    text.split(|character: char| {
                        !character.is_ascii_alphanumeric() && character != '_'
                    })
                    .filter(|word| !word.is_empty())
                    .map(str::to_owned),
                );
            }
        }
    }
    Ok(words)
}

/// Lines outside fenced code blocks, with their 1-based line numbers.
fn prose_lines(document: &str) -> Vec<(usize, &str)> {
    let mut fenced = false;
    document
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                return None;
            }
            (!fenced).then_some((index + 1, line))
        })
        .collect()
}

/// Inline code spans delimited by single backticks.
fn code_spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

/// Cited names that intentionally have no definition in this repository.
const EXTERNAL_IDENTIFIERS: [&str; 5] = [
    // Rustdoc code block attributes.
    "no_run",
    "compile_fail",
    // Variables of the removed 0.x shared daemon, named by the migration guide.
    "CAPSULE_SOCKET_PATH",
    "CAPSULE_SOCK_DIR",
    // A zsh option that Capsule never sets.
    "PROMPT_PERCENT",
];

/// Rust or shell identifiers worth resolving: paths and snake/screaming case
/// names, excluding naming prefixes such as `to_`.
fn cited_identifier(span: &str) -> Option<&str> {
    let span = span.strip_suffix("()").unwrap_or(span);
    if span.ends_with('_') || EXTERNAL_IDENTIFIERS.contains(&span) {
        return None;
    }
    let is_identifier = |segment: &str| {
        segment
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && segment
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    };
    (span.split("::").all(is_identifier) && (span.contains("::") || span.contains('_')))
        .then_some(span)
}

/// GitHub heading anchors of a Markdown document, including duplicate suffixes.
fn anchors(document: &str) -> BTreeSet<String> {
    let mut seen = BTreeMap::<String, usize>::new();
    let mut anchors = BTreeSet::new();
    for (_, line) in prose_lines(document) {
        let Some(heading) = line.strip_prefix('#') else {
            continue;
        };
        let slug = heading
            .trim_start_matches('#')
            .trim()
            .to_lowercase()
            .chars()
            .filter(|character| character.is_alphanumeric() || matches!(character, ' ' | '-' | '_'))
            .map(|character| if character == ' ' { '-' } else { character })
            .collect::<String>();
        let count = seen.entry(slug.clone()).or_default();
        anchors.insert(if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    anchors
}

/// Inline link targets, `](target)`, on one line.
fn link_targets(line: &str) -> Vec<&str> {
    line.split("](")
        .skip(1)
        .filter_map(|rest| rest.split_once(')').map(|(target, _)| target))
        .map(|target| target.split_once(' ').map_or(target, |(path, _)| path))
        .collect()
}

#[test]
fn relative_links_in_docs_resolve() -> TestResult {
    let mut broken = Vec::new();
    for path in documents()? {
        let document = fs::read_to_string(&path)?;
        let directory = path.parent().ok_or("document without a parent")?;
        for (number, line) in prose_lines(&document) {
            for target in link_targets(line) {
                if target.contains("://") || target.starts_with("mailto:") {
                    continue;
                }
                let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
                let resolved = if file.is_empty() {
                    path.clone()
                } else {
                    directory.join(file)
                };
                let valid = resolved.exists()
                    && (anchor.is_empty()
                        || resolved
                            .extension()
                            .is_some_and(|extension| extension != "md")
                        || anchors(&fs::read_to_string(&resolved)?).contains(anchor));
                if !valid {
                    broken.push(format!("{}:{number}: {target}", path.display()));
                }
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
    Ok(())
}

#[test]
fn identifiers_cited_in_docs_exist_in_sources() -> TestResult {
    let words = source_words()?;
    let mut missing = Vec::new();
    for path in documents()? {
        let document = fs::read_to_string(&path)?;
        for (number, line) in prose_lines(&document) {
            for identifier in code_spans(line).into_iter().filter_map(cited_identifier) {
                if identifier
                    .split("::")
                    .any(|segment| !words.contains(segment))
                {
                    missing.push(format!("{}:{number}: {identifier}", path.display()));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "identifiers not found in sources:\n{}",
        missing.join("\n")
    );
    Ok(())
}

/// Lint levels from `Cargo.toml` `[workspace.lints.*]` sections.
fn workspace_lint_levels(manifest: &str) -> BTreeMap<String, String> {
    let mut levels = BTreeMap::new();
    let mut in_lints = false;
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_lints = line.starts_with("[workspace.lints.");
        } else if in_lints && let Some((name, level)) = line.split_once('=') {
            levels.insert(
                name.trim().to_owned(),
                level.trim().trim_matches('"').to_owned(),
            );
        }
    }
    levels
}

/// How the docs spell each lint they promise, and its Cargo lint name.
const DOCUMENTED_LINTS: [(&str, &str); 7] = [
    ("unsafe_code", "unsafe_code"),
    ("unwrap", "unwrap_used"),
    ("expect", "expect_used"),
    ("panic!", "panic"),
    ("todo!", "todo"),
    ("unimplemented!", "unimplemented"),
    ("dbg!", "dbg_macro"),
];

#[test]
fn documented_lint_policy_matches_the_workspace_manifest() -> TestResult {
    let levels = workspace_lint_levels(&read("Cargo.toml")?);
    for document in ["AGENTS.md", "CLAUDE.md", "docs/tooling.md"] {
        let text = read(document)?;
        let mut promised = BTreeSet::new();
        for (number, line) in prose_lines(&text) {
            if !line.contains("denied") && !line.contains("forbidden") {
                continue;
            }
            for span in code_spans(line) {
                let Some((_, lint)) = DOCUMENTED_LINTS
                    .iter()
                    .find(|(spelling, _)| *spelling == span)
                else {
                    assert!(
                        !span.ends_with('!'),
                        "{document}:{number}: unmapped lint `{span}`"
                    );
                    continue;
                };
                let expected = if *lint == "unsafe_code" {
                    "forbid"
                } else {
                    "deny"
                };
                assert_eq!(
                    levels.get(*lint).map(String::as_str),
                    Some(expected),
                    "{document}:{number}: `{span}`"
                );
                promised.insert(*lint);
            }
        }
        assert!(promised.len() > 1, "{document}: no lint policy found");
    }
    Ok(())
}
