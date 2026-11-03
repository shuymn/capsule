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
const EXTERNAL_IDENTIFIERS: [&str; 6] = [
    // The standard library and Tokio process builder.
    "Command::new",
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

/// Sources, scripts, configuration, and root files that define cited names,
/// excluding this test so its allowlists cannot satisfy themselves.
fn source_texts() -> Result<Vec<(PathBuf, String)>, std::io::Error> {
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
    files.retain(|path| *path != this_test && !is_markdown(path));
    // Binary files and bytecode caches carry no cited names.
    Ok(files
        .into_iter()
        .filter_map(|path| fs::read_to_string(&path).ok().map(|text| (path, text)))
        .collect())
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|word| !word.is_empty())
}

/// The self type named by an `impl` header, after any generics and `for`.
fn impl_type(header: &str) -> Option<String> {
    let mut rest = header.trim_start();
    if rest.starts_with('<') {
        let mut depth = 0_usize;
        let end = rest.char_indices().find_map(|(index, character)| {
            match character {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(index + 1)
        })?;
        rest = &rest[end..];
    }
    let rest = rest.rsplit_once(" for ").map_or(rest, |(_, target)| target);
    let path = rest
        .trim_start()
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != ':'
        })
        .next()?;
    path.rsplit("::")
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// `Type::function` for every `fn` inside an `impl` block of rustfmt-formatted
/// Rust, whose closing brace sits at the indentation of its header.
fn impl_functions(source: &str) -> BTreeSet<String> {
    let mut functions = BTreeSet::new();
    let mut open = Vec::<(&str, String)>::new();
    for line in source.lines() {
        let item = line.trim_start();
        let indent = &line[..line.len() - item.len()];
        if item == "}"
            && open
                .last()
                .is_some_and(|(open_indent, _)| *open_indent == indent)
        {
            open.pop();
        } else if let Some(header) = item.strip_prefix("impl")
            && header.starts_with(['<', ' '])
            && let Some(type_name) = impl_type(header)
        {
            open.push((indent, type_name));
        } else if let Some((_, type_name)) = open.last() {
            let signature = ["pub(crate) ", "pub(super) ", "pub ", "const ", "async "]
                .iter()
                .fold(item, |rest, qualifier| {
                    rest.strip_prefix(qualifier).unwrap_or(rest)
                });
            if let Some(name) = signature
                .strip_prefix("fn ")
                .and_then(|rest| words(rest).next())
            {
                functions.insert(format!("{type_name}::{name}"));
            }
        }
    }
    functions
}

/// The Rust crate name of the workspace member whose directory holds `path`.
fn crate_name(path: &Path) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let crates = root().join("crates");
    let Some(member) = path
        .strip_prefix(&crates)
        .ok()
        .and_then(|relative| relative.components().next())
    else {
        return Ok(None);
    };
    let manifest = fs::read_to_string(crates.join(member).join("Cargo.toml"))?;
    let name = manifest.parse::<toml::Table>()?["package"]["name"]
        .as_str()
        .map(|name| name.replace('-', "_"));
    Ok(name)
}

/// Names that cited identifiers may resolve to: every word, and every
/// `Type::function` defined in an `impl` block, also qualified by the name of
/// the crate that defines it.
fn source_names() -> Result<(BTreeSet<String>, BTreeSet<String>), Box<dyn std::error::Error>> {
    let mut all_words = BTreeSet::new();
    let mut functions = BTreeSet::new();
    for (path, text) in source_texts()? {
        all_words.extend(words(&text).map(str::to_owned));
        if path.extension().is_some_and(|extension| extension == "rs") {
            let crate_name = crate_name(&path)?;
            all_words.extend(crate_name.clone());
            for function in impl_functions(&text) {
                if let Some(crate_name) = &crate_name {
                    functions.insert(format!("{crate_name}::{function}"));
                }
                functions.insert(function);
            }
        }
    }
    Ok((all_words, functions))
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

/// Whether `character` is in a combining-mark block, which GitHub keeps in
/// heading anchors; std has no general-category lookup.
fn is_combining_mark(character: char) -> bool {
    matches!(
        character,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{3099}'..='\u{309A}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// GitHub heading anchors of a Markdown document, including duplicate
/// suffixes: each heading's rendered text, lowercased, without punctuation,
/// and with whitespace as `-`. Headings with HTML or images are rejected,
/// because GitHub sanitizes them before slugging.
fn anchors(document: &str) -> Result<BTreeSet<String>, String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    // Suffix counts per base slug, as github-slugger keeps them.
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut anchors = BTreeSet::new();
    let mut heading = None::<String>;
    let parser = Parser::new_ext(document, Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    for event in parser {
        match (&mut heading, event) {
            (None, Event::Start(Tag::Heading { .. })) => heading = Some(String::new()),
            (Some(text), Event::Text(part) | Event::Code(part)) => text.push_str(&part),
            (Some(text), Event::Html(markup) | Event::InlineHtml(markup)) => {
                return Err(format!("unsupported HTML {markup:?} in heading: {text}"));
            }
            (Some(text), Event::Start(Tag::Image { .. })) => {
                return Err(format!("unsupported image in heading: {text}"));
            }
            (Some(text), Event::End(TagEnd::Heading(_))) => {
                let base = text
                    .to_lowercase()
                    .chars()
                    .filter(|character| {
                        character.is_alphanumeric()
                            || character.is_whitespace()
                            || is_combining_mark(*character)
                            || matches!(character, '-' | '_')
                    })
                    .map(|character| {
                        if character.is_whitespace() {
                            '-'
                        } else {
                            character
                        }
                    })
                    .collect::<String>();
                // A suffixed slug may collide with an earlier heading's own
                // text, as `Foo`, `Foo`, `Foo-1` does; skip issued slugs.
                let mut slug = base.clone();
                while occurrences.contains_key(&slug) {
                    let count = occurrences.entry(base.clone()).or_default();
                    *count += 1;
                    slug = format!("{base}-{count}");
                }
                occurrences.insert(slug.clone(), 0);
                anchors.insert(slug);
                heading = None;
            }
            _ => {}
        }
    }
    Ok(anchors)
}

#[test]
fn anchors_slug_rendered_heading_text() -> TestResult {
    let document = "---\ntitle: x\n---\n\
        # Setup\n## MAX_REQUEST bound\n   ### Setup ##\n\
        ## [Install](setup.md) `cargo_add`\n## *Emphasis* and **strong**\n\
        ## Type \\[T\\] \\*literal\\*\n## A &amp; B\n## **Unmatched\n\
        Setext title\n============\n\
        Intro\n```\n# not a heading\n```\n---\nIntro\n\t---\n\
        # Foo\n# Foo\n# Foo-1\n## Cafe\u{301}\n";
    assert_eq!(
        anchors(document)?,
        [
            "setup",
            "max_request-bound",
            "setup-1",
            "install-cargo_add",
            "emphasis-and-strong",
            "type-t-literal",
            "a--b",
            "unmatched",
            "setext-title",
            "foo",
            "foo-1",
            "foo-1-1",
            "cafe\u{301}",
        ]
        .map(str::to_owned)
        .into()
    );
    for unsupported in ["## <em>HTML</em>\n", "## ![Logo](logo.png) Name\n"] {
        assert!(anchors(unsupported).is_err(), "{unsupported}");
    }
    Ok(())
}

/// Anchors of the Markdown file at `path`, with the path in any error.
fn anchors_at(path: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let document = fs::read_to_string(path)?;
    Ok(anchors(&document).map_err(|error| format!("{}: {error}", path.display()))?)
}

/// Link targets on one line: Markdown `](target)` and HTML `src="target"` or
/// `href="target"`.
fn link_targets(line: &str) -> impl Iterator<Item = &str> {
    let markdown = line
        .split("](")
        .skip(1)
        .filter_map(|rest| rest.split_once(')').map(|(target, _)| target))
        .map(|target| target.split_once(' ').map_or(target, |(path, _)| path));
    let html = [" src=\"", " href=\""]
        .into_iter()
        .flat_map(move |attribute| {
            line.split(attribute)
                .skip(1)
                .filter_map(|rest| rest.split_once('"').map(|(target, _)| target))
        });
    markdown.chain(html)
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

/// Body rows of the Markdown table whose header line is `header`.
fn table_rows<'a>(document: &'a str, header: &str) -> impl Iterator<Item = &'a str> {
    document
        .lines()
        .skip_while(move |line| *line != header)
        .skip(2)
        .take_while(|line| line.starts_with('|'))
}

/// Rows of the `| Boundary | Limit | Constant |` table, keyed by constant.
fn documented_limits(document: &str) -> Result<BTreeMap<String, Limit>, String> {
    let mut rows = BTreeMap::new();
    for line in table_rows(document, "| Boundary | Limit | Constant |") {
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
    // Slug every document first, so an unsupported heading fails even when no
    // link targets it; other Markdown targets are slugged when first linked.
    let mut anchors_by_path = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for path in &documents {
        anchors_by_path.insert(path.canonicalize()?, anchors_at(path)?);
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
                let resolved = if file.is_empty() {
                    path.canonicalize().ok()
                } else {
                    directory.join(file).canonicalize().ok()
                };
                let valid = match resolved {
                    None => false,
                    Some(_) if anchor.is_empty() => true,
                    Some(resolved) if is_markdown(&resolved) => {
                        if !anchors_by_path.contains_key(&resolved) {
                            let headings = anchors_at(&resolved)?;
                            anchors_by_path.insert(resolved.clone(), headings);
                        }
                        anchors_by_path
                            .get(&resolved)
                            .is_some_and(|anchors| anchors.contains(anchor))
                    }
                    Some(_) => true,
                };
                if !valid {
                    broken.push(format!("{}:{number}: {target}", path.display()));
                }
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
    Ok(())
}

/// Whether a cited identifier names something defined in the sources: every
/// segment must occur, and a trailing `Type::function` must be defined in that
/// type's `impl` block, inside the named crate when the path starts with one.
/// Module segments are checked only as words.
fn resolves(identifier: &str, words: &BTreeSet<String>, functions: &BTreeSet<String>) -> bool {
    let segments = identifier.split("::").collect::<Vec<_>>();
    let defined = segments.iter().all(|segment| words.contains(*segment));
    match segments.as_slice() {
        [.., type_name, function]
            if type_name.starts_with(|first: char| first.is_ascii_uppercase())
                && function.starts_with(|first: char| first.is_ascii_lowercase()) =>
        {
            let method = format!("{type_name}::{function}");
            let method = match segments.as_slice() {
                [crate_name, _, _, ..]
                    if functions
                        .range(format!("{crate_name}::")..)
                        .next()
                        .is_some_and(|first| first.starts_with(&format!("{crate_name}::"))) =>
                {
                    format!("{crate_name}::{method}")
                }
                _ => method,
            };
            defined && functions.contains(&method)
        }
        _ => defined,
    }
}

#[test]
fn crate_names_come_from_member_manifests() -> TestResult {
    let cli = crate_name(&root().join("crates/cli/tests/docs.rs"))?;
    assert_eq!(cli.as_deref(), Some("capsule_cli"));
    assert_eq!(crate_name(&root().join("scripts/x"))?, None);
    Ok(())
}

#[test]
fn method_paths_resolve_by_their_final_type_and_function() {
    let words = [
        "capsule_core",
        "plan",
        "ConfigPlan",
        "parse",
        "render",
        "Limit",
        "Count",
        "capsule_prompt_bench",
        "worker",
        "Worker",
        "measure",
    ]
    .map(str::to_owned)
    .into();
    let functions = [
        "ConfigPlan::parse",
        "capsule_core::ConfigPlan::parse",
        "Worker::measure",
        "capsule_prompt_bench::Worker::measure",
    ]
    .map(str::to_owned)
    .into();
    for cited in [
        "ConfigPlan::parse",
        "capsule_core::ConfigPlan::parse",
        "capsule_core::plan::ConfigPlan::parse",
        "Limit::Count",
        "capsule_core::Limit::Count",
        "capsule_prompt_bench::worker::Worker::measure",
    ] {
        assert!(resolves(cited, &words, &functions), "{cited}");
    }
    for cited in [
        "ConfigPlan::render",
        "capsule_core::ConfigPlan::render",
        "missing::ConfigPlan::parse",
        "capsule_core::Limit::Bytes",
        // Defined in another crate.
        "capsule_core::worker::Worker::measure",
    ] {
        assert!(!resolves(cited, &words, &functions), "{cited}");
    }
}

#[test]
fn identifiers_cited_in_docs_exist_in_sources() -> TestResult {
    let (words, functions) = source_names()?;
    let mut missing = Vec::new();
    for path in documents()? {
        let document = fs::read_to_string(&path)?;
        for (number, line) in prose_lines(&document) {
            for identifier in code_spans(line).filter_map(cited_identifier) {
                if !resolves(identifier, &words, &functions) {
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

#[test]
fn every_workspace_member_inherits_the_workspace_lints() -> TestResult {
    let mut members = 0;
    for entry in fs::read_dir(root().join("crates"))? {
        let manifest = entry?.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let manifest_table = fs::read_to_string(&manifest)?.parse::<toml::Table>()?;
        let inherits = manifest_table
            .get("lints")
            .and_then(|lints| lints.get("workspace"))
            .and_then(toml::Value::as_bool);
        assert_eq!(
            inherits,
            Some(true),
            "{} must set [lints] workspace = true",
            manifest.display()
        );
        members += 1;
    }
    assert!(members > 0, "no workspace members found");
    Ok(())
}

/// A `Covers:` comment naming the threats a test, or a whole test file, verifies.
#[derive(Debug, PartialEq, Eq)]
struct Coverage {
    path: String,
    /// `None` when the tag sits in the file's leading comments.
    test: Option<String>,
    threats: Vec<String>,
}

/// The body of a Rust, Python, or shell comment line, or of an attribute.
fn comment_body(line: &str) -> Option<&str> {
    let line = line.trim_start();
    ["///", "//!", "//", "#"]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(str::trim)
}

/// The name of a Rust `fn` or Python `def` declared on `line`.
fn declared_function(line: &str) -> Option<&str> {
    let mut words = words(line);
    loop {
        match words.next()? {
            "pub" | "crate" | "super" | "const" | "async" => {}
            "fn" | "def" => return words.next(),
            _ => return None,
        }
    }
}

/// `Covers:` tags in one source file. A tag binds to the function declared
/// after its run of comments, attributes, and decorators (a Rust test needs a
/// `#[test]` or `#[tokio::test]` attribute in that run), or, in a shell
/// script, to the file when it precedes all code; any other placement, or a
/// threat outside `threats`, is an error.
fn coverage_tags(
    path: &str,
    source: &str,
    threats: &BTreeSet<&str>,
) -> Result<Vec<Coverage>, String> {
    let rust = path.ends_with(".rs");
    let mut tags = Vec::new();
    let mut pending = Vec::new();
    let mut is_test = false;
    let mut header = true;
    for (index, line) in source.lines().enumerate() {
        let location = format!("{path}:{}", index + 1);
        if let Some(body) = comment_body(line) {
            is_test |=
                matches!(body, "[test]" | "[tokio::test]") || body.starts_with("[tokio::test(");
            if let Some(list) = body.strip_prefix("Covers:") {
                let ids = list.split(',').map(str::trim).collect::<Vec<_>>();
                if let Some(id) = ids.iter().find(|id| !threats.contains(*id)) {
                    return Err(format!(
                        "{location}: unknown threat {id:?} in `Covers:` tag"
                    ));
                }
                pending.push((location, ids.into_iter().map(str::to_owned).collect()));
            }
            continue;
        }
        if line.trim_start().starts_with('@') {
            continue;
        }
        let declared = declared_function(line).filter(|_| is_test || !rust);
        for (location, threats) in pending.drain(..) {
            let test = match declared {
                Some(name) => Some(name.to_owned()),
                None if header && path.ends_with(".sh") => None,
                None => return Err(format!("{location}: `Covers:` tag precedes no test")),
            };
            tags.push(Coverage {
                path: path.to_owned(),
                test,
                threats,
            });
        }
        is_test = false;
        header &= line.trim().is_empty();
    }
    match pending.first() {
        Some((location, _)) => Err(format!("{location}: `Covers:` tag precedes no test")),
        None => Ok(tags),
    }
}

/// Names a clause may cite: Rust tests, the clause checks that take a model
/// `Step`, and the records quoted in the PTY contract registry.
fn specification_names() -> Result<BTreeSet<String>, std::io::Error> {
    let mut names = BTreeSet::new();
    for (path, text) in source_texts()? {
        if path.ends_with("scripts/pty_suite/contracts.py") {
            let quoted = text.split('"').skip(1).step_by(2);
            names.extend(
                quoted
                    .filter(|name| is_identifier(name) && name.contains('_'))
                    .map(str::to_owned),
            );
        }
        let mut is_test = false;
        let mut rest = text.as_str();
        while let Some((line, tail)) = rest.split_once('\n') {
            rest = tail;
            if let Some(body) = comment_body(line) {
                is_test |= body == "[test]" || body.starts_with("[tokio::test");
                continue;
            }
            // A wrapped signature continues up to the body's opening brace.
            let wrapped = tail.split_once('{').filter(|_| !line.contains('{'));
            let is_clause = format!("{line}{}", wrapped.map_or("", |(signature, _)| signature))
                .contains("step: &Step");
            if let Some(name) = declared_function(line).filter(|_| is_test || is_clause) {
                names.insert(name.to_owned());
            }
            is_test = false;
        }
    }
    Ok(names)
}

#[test]
fn behavioral_contract_clauses_cite_declared_tests() -> TestResult {
    let names = specification_names()?;
    let document = read("docs/architecture.md")?;
    let section = document
        .split_once("## Behavioral contract\n")
        .and_then(|(_, rest)| rest.split("\n## ").next())
        .ok_or("missing Behavioral contract section")?;
    let mut problems = Vec::new();
    for clause in section.lines().filter(|line| line.starts_with("- ")) {
        let cited: Vec<_> = clause
            .split_once(" Spec: ")
            .map(|(_, spec)| code_spans(spec).collect())
            .unwrap_or_default();
        if cited.is_empty() {
            problems.push(format!("cites no test: {clause}"));
        }
        for name in cited.into_iter().filter(|name| !names.contains(*name)) {
            problems.push(format!("not a test, clause check, or PTY record: {name}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    Ok(())
}

/// Function names listed in the PTY suite's run-order `SCENARIOS` tuple.
fn registered_scenarios(source: &str) -> BTreeSet<&str> {
    source
        .lines()
        .skip_while(|line| *line != "SCENARIOS = (")
        .skip(1)
        .take_while(|line| *line != ")")
        .map(|line| line.trim().trim_end_matches(','))
        .collect()
}

/// Threat IDs defined by the `### Txx — …` headings of the threat model.
fn defined_threats(document: &str) -> BTreeSet<&str> {
    prose_lines(document)
        .into_iter()
        .filter_map(|(_, line)| line.strip_prefix("### "))
        .filter_map(|heading| heading.split_whitespace().next())
        .filter(|id| id.starts_with('T'))
        .collect()
}

/// The generated "Existing verification" cell for a row covering `threats`:
/// the tagged tests by file, then each threat that has none.
fn verification_cell(tags: &[Coverage], threats: &BTreeSet<&str>) -> String {
    let mut files = BTreeMap::<&str, Vec<&str>>::new();
    let mut untagged = threats.clone();
    for tag in tags {
        if !tag
            .threats
            .iter()
            .any(|threat| threats.contains(threat.as_str()))
        {
            continue;
        }
        for threat in &tag.threats {
            untagged.remove(threat.as_str());
        }
        files
            .entry(&tag.path)
            .or_default()
            .extend(tag.test.as_deref());
    }
    let mut parts = files
        .into_iter()
        .map(|(path, tests)| {
            let link = format!("[{path}](../{path})");
            if tests.is_empty() {
                return link;
            }
            let names = tests
                .iter()
                .map(|test| format!("`{test}`"))
                .collect::<Vec<_>>();
            format!("{link}: {}", names.join(", "))
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return "Source review only".to_owned();
    }
    if !untagged.is_empty() {
        let untagged = untagged.into_iter().collect::<Vec<_>>();
        parts.push(format!("source review only for {}", untagged.join(", ")));
    }
    parts.join("; ")
}

/// `docs/threat-model.md` with every §8 "Existing verification" cell
/// regenerated from `tags`.
fn regenerate_verification(document: &str, tags: &[Coverage]) -> Result<String, String> {
    let defined = defined_threats(document);
    let mut listed = BTreeSet::new();
    let mut updated = document.to_owned();
    let header = "| Threat / condition | Existing verification | Additional checks and evidence |";
    for row in table_rows(document, header) {
        let cells = row
            .strip_prefix("| ")
            .and_then(|row| row.strip_suffix(" |"))
            .map(|row| row.splitn(3, " | ").collect::<Vec<_>>());
        let Some([condition, _, additional]) = cells.as_deref() else {
            return Err(format!("malformed verification row: {row}"));
        };
        let threats = words(condition)
            .filter(|word| defined.contains(word))
            .collect::<BTreeSet<_>>();
        for threat in &threats {
            if !listed.insert(*threat) {
                return Err(format!("{threat} has more than one verification row"));
            }
        }
        let cell = verification_cell(tags, &threats);
        updated = updated.replacen(row, &format!("| {condition} | {cell} | {additional} |"), 1);
    }
    if listed != defined {
        return Err(format!(
            "threats without a verification row: {:?}",
            defined.difference(&listed)
        ));
    }
    Ok(updated)
}

#[test]
fn coverage_tags_bind_to_the_next_test_or_the_file_header() -> TestResult {
    let threats = BTreeSet::from(["T01", "T07", "T08", "T10"]);
    let tag = |test: Option<&str>, threats: &[&str]| Coverage {
        path: "a.rs".into(),
        test: test.map(str::to_owned),
        threats: threats.iter().map(|threat| (*threat).to_owned()).collect(),
    };
    let script = coverage_tags("a.sh", "#!/bin/sh\n# Covers: T10\nset -eu\n", &threats)?;
    assert_eq!(
        script,
        [Coverage {
            path: "a.sh".into(),
            test: None,
            threats: vec!["T10".into()]
        }]
    );
    let source = "use x;\n\n\
        /// Covers: T01, T07\n#[test]\n/// Docs.\nfn first() {}\n\
        // Covers: T08\n#[tokio::test]\nasync fn second() {}\n";
    assert_eq!(
        coverage_tags("a.rs", source, &threats)?,
        [
            tag(Some("first"), &["T01", "T07"]),
            tag(Some("second"), &["T08"])
        ]
    );
    let python = coverage_tags(
        "a.py",
        "x = 1\n# Covers: T08\n@decorator\ndef run(suite):\n",
        &threats,
    )?;
    assert_eq!(
        python.first().and_then(|tag| tag.test.as_deref()),
        Some("run")
    );
    for stray in [
        "use x;\n// Covers: T01\nconst X: u8 = 0;\n",
        "use x;\n/// Covers: T01\nfn helper() {}\n",
        "/// Covers: T01,\n#[test]\nfn f() {}\n",
        "/// Covers: T11\n#[test]\nfn f() {}\n",
        "use x;\n// Covers: T01\n",
        "// Covers: T01\nuse x;\n",
        "use x;\n/// Covers: T01\n#[cfg(test)]\nfn helper() {}\n",
    ] {
        assert!(coverage_tags("a.rs", stray, &threats).is_err(), "{stray}");
    }
    Ok(())
}

#[test]
fn verification_cells_list_each_threat_without_a_tagged_test() {
    let tags = [Coverage {
        path: "a.rs".into(),
        test: Some("t".into()),
        threats: vec!["T02".into()],
    }];
    assert_eq!(
        verification_cell(&tags, &BTreeSet::from(["T02", "T06"])),
        "[a.rs](../a.rs): `t`; source review only for T06"
    );
    assert_eq!(
        verification_cell(&tags, &BTreeSet::from(["T05"])),
        "Source review only"
    );
}

/// Set `CAPSULE_UPDATE_DOCS=1` to rewrite the column instead of failing.
#[test]
fn threat_model_verification_column_matches_coverage_tags() -> TestResult {
    let path = "docs/threat-model.md";
    let document = read(path)?;
    let threats = defined_threats(&document);
    let mut tags = Vec::new();
    for (source, text) in source_texts()? {
        if matches!(
            source.extension().and_then(|extension| extension.to_str()),
            Some("rs" | "py" | "sh")
        ) {
            let relative = source.strip_prefix(root())?.to_string_lossy().into_owned();
            tags.extend(coverage_tags(&relative, &text, &threats)?);
        }
    }
    let scenarios = read("scripts/pty_suite/scenarios/__init__.py")?;
    let registered = registered_scenarios(&scenarios);
    for tag in tags.iter().filter(|tag| tag.path.ends_with(".py")) {
        let test = tag.test.as_deref().unwrap_or_default();
        assert!(
            registered.contains(test),
            "{}: `{test}` is not in SCENARIOS",
            tag.path
        );
    }
    let updated = regenerate_verification(&document, &tags)?;
    if updated != document && std::env::var_os("CAPSULE_UPDATE_DOCS").is_some() {
        fs::write(root().join(path), &updated)?;
        return Ok(());
    }
    assert!(
        updated == document,
        "{path} §8 is out of date with the `Covers:` tags; run \
         `CAPSULE_UPDATE_DOCS=1 cargo test -p capsule-cli --test docs`"
    );
    Ok(())
}
