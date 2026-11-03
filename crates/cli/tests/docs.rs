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
