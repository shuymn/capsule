//! Keep documentation that restates code facts in sync with the code.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use capsule_core::limits::{self, Limit};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Directories whose files define the names that documentation cites.
const SOURCE_DIRECTORIES: [&str; 5] = ["crates", "scripts", "nix", "examples", ".github"];

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

/// Documentation spellings of lints whose Cargo name differs.
const LINT_ALIASES: [(&str, &str); 6] = [
    ("unwrap", "unwrap_used"),
    ("expect", "expect_used"),
    ("panic!", "panic"),
    ("todo!", "todo"),
    ("unimplemented!", "unimplemented"),
    ("dbg!", "dbg_macro"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> Result<String, std::io::Error> {
    fs::read_to_string(root().join(path))
}

/// Regular files under `directory`, without following symlinks.
fn files_under(directory: PathBuf) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    let mut directories = vec![directory];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

/// Markdown written for agents and users: `docs/` and the root documents.
fn documents() -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = files_under(root().join("docs"))?;
    files.retain(|path| is_markdown(path));
    files.extend(
        ["AGENTS.md", "CLAUDE.md", "README.md", "README.ja.md"]
            .iter()
            .map(|name| root().join(name)),
    );
    files.sort();
    Ok(files)
}

/// Every identifier-like word in sources, scripts, configuration, and root
/// files, excluding this test so its allowlists cannot satisfy themselves.
fn source_words() -> Result<BTreeSet<String>, std::io::Error> {
    let this_test = root().join("crates/cli/tests/docs.rs");
    let mut files = Vec::new();
    for directory in SOURCE_DIRECTORIES {
        files.extend(files_under(root().join(directory))?);
    }
    for entry in fs::read_dir(root())? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    let mut words = BTreeSet::new();
    for path in files {
        if path == this_test || is_markdown(&path) {
            continue;
        }
        // Binary files and bytecode caches carry no cited names.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        words.extend(
            text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .filter(|word| !word.is_empty())
                .map(str::to_owned),
        );
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
fn code_spans(line: &str) -> impl Iterator<Item = &str> {
    line.split('`').skip(1).step_by(2)
}

fn is_identifier(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Rust or shell identifiers worth resolving: paths and snake/screaming case
/// names, excluding naming prefixes such as `to_`.
fn cited_identifier(span: &str) -> Option<&str> {
    let span = span.strip_suffix("()").unwrap_or(span);
    let resolvable = span.split("::").all(is_identifier)
        && (span.contains("::") || span.contains('_'))
        && !span.ends_with('_')
        && !EXTERNAL_IDENTIFIERS.contains(&span);
    resolvable.then_some(span)
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
fn link_targets(line: &str) -> impl Iterator<Item = &str> {
    line.split("](")
        .skip(1)
        .filter_map(|rest| rest.split_once(')').map(|(target, _)| target))
        .map(|target| target.split_once(' ').map_or(target, |(path, _)| path))
}

fn parse_limit(text: &str) -> Option<Limit> {
    let (number, unit) = text.split_once(' ').unwrap_or((text, ""));
    let number = number.parse::<usize>().ok()?;
    let millis = |factor: u64| Some(Duration::from_millis(u64::try_from(number).ok()? * factor));
    Some(match unit {
        "" => Limit::Count(number),
        "bytes" => Limit::Bytes(number),
        "KiB" => Limit::Bytes(number * 1024),
        "ms" => Limit::Time(millis(1)?),
        "s" => Limit::Time(millis(1000)?),
        _ => return None,
    })
}

/// Rows of the `| Boundary | Limit | Constant |` table, keyed by constant.
fn documented_limits(document: &str) -> Result<BTreeMap<String, Limit>, String> {
    let mut rows = BTreeMap::new();
    let lines = document
        .lines()
        .skip_while(|line| *line != "| Boundary | Limit | Constant |")
        .skip(2)
        .take_while(|line| line.starts_with('|'));
    for line in lines {
        // Split from the right so boundary text may contain `|`.
        let cells = line
            .trim_matches('|')
            .rsplitn(3, '|')
            .map(str::trim)
            .collect::<Vec<_>>();
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
fn threat_model_limits_table_matches_the_limits_module() -> TestResult {
    let documented = documented_limits(&read("docs/threat-model.md")?)?;
    let defined = limits::ALL
        .iter()
        .map(|(name, limit)| ((*name).to_owned(), *limit))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(defined.len(), limits::ALL.len(), "duplicate limit names");
    assert_eq!(documented, defined);
    Ok(())
}

#[test]
fn relative_links_in_docs_resolve() -> TestResult {
    let documents = documents()?;
    let mut anchors_by_path = BTreeMap::new();
    for path in &documents {
        anchors_by_path.insert(path.canonicalize()?, anchors(&fs::read_to_string(path)?));
    }
    let mut broken = Vec::new();
    for path in &documents {
        let document = fs::read_to_string(path)?;
        let directory = path.parent().ok_or("document without a parent")?;
        for (number, line) in prose_lines(&document) {
            let local = link_targets(line)
                .filter(|target| !target.contains("://") && !target.starts_with("mailto:"));
            for target in local {
                let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
                let resolved = directory.join(file).canonicalize().ok();
                // Anchors are checked in documents; links into other files may name lines.
                let valid = resolved.is_some_and(|resolved| {
                    anchor.is_empty()
                        || anchors_by_path
                            .get(&resolved)
                            .is_none_or(|anchors| anchors.contains(anchor))
                });
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
            for identifier in code_spans(line).filter_map(cited_identifier) {
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

/// Lint levels from `Cargo.toml` `[workspace.lints.*]`, in short or table form.
fn workspace_lint_levels() -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let manifest = read("Cargo.toml")?.parse::<toml::Table>()?;
    let groups = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(toml::Value::as_table)
        .ok_or("missing [workspace.lints]")?;
    let mut levels = BTreeMap::new();
    for lints in groups.values().filter_map(toml::Value::as_table) {
        for (name, setting) in lints {
            let level = setting
                .as_str()
                .or_else(|| setting.get("level").and_then(toml::Value::as_str))
                .ok_or("lint without a level")?;
            levels.insert(name.clone(), level.to_owned());
        }
    }
    Ok(levels)
}

#[test]
fn documented_lint_policy_matches_the_workspace_manifest() -> TestResult {
    let levels = workspace_lint_levels()?;
    for document in ["AGENTS.md", "CLAUDE.md", "docs/tooling.md"] {
        let text = read(document)?;
        let mut promised = 0;
        let policy_lines = prose_lines(&text)
            .into_iter()
            .filter(|(_, line)| line.contains("denied") || line.contains("forbidden"));
        for (number, line) in policy_lines {
            for span in code_spans(line).filter(|span| is_identifier(span.trim_end_matches('!'))) {
                let lint = LINT_ALIASES
                    .iter()
                    .find(|(spelling, _)| *spelling == span)
                    .map_or(span, |(_, lint)| lint);
                let level = levels.get(lint).map(String::as_str);
                assert!(
                    matches!(level, Some("deny" | "forbid")),
                    "{document}:{number}: `{span}` is {level:?} in Cargo.toml"
                );
                promised += 1;
            }
        }
        assert!(promised > 1, "{document}: no lint policy found");
    }
    Ok(())
}
