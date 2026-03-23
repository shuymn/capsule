use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use regex_lite::Regex;
use tokio::task::JoinSet;

use super::{
    super::ModuleSpeed,
    CustomModuleInfo, ModuleDependencyInputs, RequestFacts, ResolvedModule, ResolvedSource,
    ResolvedSourceGroup,
    detect::{apply_regex, format_module},
};
use crate::config::ModuleWhen;

impl ModuleDependencyInputs {
    fn push_env(&mut self, name: &str) {
        push_unique(&mut self.env_vars, name);
    }

    fn push_file(&mut self, path: &str) {
        push_unique(&mut self.files, path);
    }

    /// Env var values come from the request; file dependencies are resolved as
    /// existence checks against the request's cwd.
    #[must_use]
    pub(crate) fn compute_dep_hash(&self, facts: &RequestFacts) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        let mut env_sorted: Vec<&str> = self.env_vars.iter().map(String::as_str).collect();
        env_sorted.sort_unstable();
        for name in &env_sorted {
            name.hash(&mut hasher);
            match facts.env_value(name) {
                Some(value) => {
                    true.hash(&mut hasher);
                    value.hash(&mut hasher);
                }
                None => false.hash(&mut hasher),
            }
        }

        let mut files_sorted: Vec<&str> = self.files.iter().map(String::as_str).collect();
        files_sorted.sort_unstable();
        for file in &files_sorted {
            file.hash(&mut hasher);
            facts.cwd().join(file).is_file().hash(&mut hasher);
        }

        hasher.finish()
    }

    fn add_module(&mut self, module: &ResolvedModule) {
        for env_name in &module.when.env {
            self.push_env(env_name);
        }
        for file_path in &module.when.files {
            self.push_file(file_path);
        }
        for source in module.all_sources() {
            match source {
                ResolvedSource::Env { name, .. } => self.push_env(name),
                ResolvedSource::File { path, .. } => self.push_file(path),
                ResolvedSource::Command { .. } => {}
            }
        }
    }
}

impl RequestFacts {
    #[must_use]
    pub(crate) fn collect(cwd: impl Into<PathBuf>, env_vars: Vec<(String, String)>) -> Self {
        let cwd = cwd.into();
        let read_only = std::fs::metadata(&cwd).is_ok_and(|meta| meta.permissions().readonly());

        Self {
            cwd,
            env_vars,
            path_env: None,
            read_only,
        }
    }

    #[must_use]
    pub(crate) fn with_command_path_env(mut self, path_env: Option<String>) -> Self {
        self.path_env = path_env;
        self
    }

    #[must_use]
    pub(crate) fn with_forwarded_path_env(mut self) -> Self {
        self.path_env = self.env_value("PATH").map(ToOwned::to_owned);
        self
    }

    #[must_use]
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    #[must_use]
    pub(crate) fn command_path_env(&self) -> Option<&str> {
        self.path_env.as_deref()
    }

    #[must_use]
    pub(crate) const fn read_only(&self) -> bool {
        self.read_only
    }

    #[must_use]
    pub(crate) fn matching_modules<'a>(
        &'a self,
        defs: &'a [ResolvedModule],
        speed: ModuleSpeed,
    ) -> Vec<(usize, &'a ResolvedModule)> {
        defs.iter()
            .filter(|module| module.speed == speed)
            .filter(|module| self.check_when(&module.when))
            .enumerate()
            .collect()
    }

    #[must_use]
    pub(crate) fn matching_dependency_inputs(
        &self,
        defs: &[ResolvedModule],
        speed: ModuleSpeed,
    ) -> ModuleDependencyInputs {
        let mut inputs = ModuleDependencyInputs::default();

        for (_, module) in self.matching_modules(defs, speed) {
            inputs.add_module(module);
        }

        inputs
    }

    #[must_use]
    pub(crate) fn check_when(&self, when: &ModuleWhen) -> bool {
        let files_ok =
            when.files.is_empty() || when.files.iter().any(|file| self.cwd.join(file).is_file());
        let env_ok = when.env.is_empty()
            || when
                .env
                .iter()
                .any(|env_name| self.env_value(env_name).is_some());
        files_ok && env_ok
    }

    /// Detects a module by resolving each source group independently
    /// and formatting with all resolved variables.
    pub(crate) async fn detect_module(&self, def: &ResolvedModule) -> Option<CustomModuleInfo> {
        let mut values = HashMap::new();
        for group in &def.source_groups {
            if let Some(raw) = self.resolve_group(group).await {
                values.insert(group.name.as_str(), raw);
            }
        }
        format_module(def, &values)
    }

    async fn resolve_group(&self, group: &ResolvedSourceGroup) -> Option<String> {
        let fast_sources: Vec<&ResolvedSource> = group
            .sources
            .iter()
            .filter(|source| source.is_fast())
            .collect();
        if let Some(raw) = self.resolve_sources(&fast_sources).await {
            return Some(raw);
        }

        let slow_sources: Vec<&ResolvedSource> = group
            .sources
            .iter()
            .filter(|source| !source.is_fast())
            .collect();
        self.resolve_sources(&slow_sources).await
    }

    async fn resolve_sources(&self, sources: &[&ResolvedSource]) -> Option<String> {
        match sources {
            [] => None,
            [source] => self.resolve_source(source).await,
            _ => {
                let mut join_set = JoinSet::new();
                for source in sources {
                    let cwd = self.cwd.clone();
                    let env_vars = self.env_vars.clone();
                    let path_env = self.command_path_env().map(ToOwned::to_owned);
                    let source = (*source).clone();
                    join_set.spawn(async move {
                        resolve_source_ref(&cwd, &env_vars, path_env.as_deref(), &source).await
                    });
                }

                while let Some(joined) = join_set.join_next().await {
                    if let Ok(Some(raw)) = joined {
                        join_set.abort_all();
                        return Some(raw);
                    }
                }

                None
            }
        }
    }

    fn env_value(&self, name: &str) -> Option<&str> {
        find_env_value(&self.env_vars, name)
    }

    async fn resolve_source(&self, source: &ResolvedSource) -> Option<String> {
        resolve_source_ref(&self.cwd, &self.env_vars, self.command_path_env(), source).await
    }
}

/// Collect all environment variable names referenced by resolved modules.
///
/// Includes variables from `when.env` and env-type sources. Deduplicated.
#[must_use]
pub fn required_env_var_names(modules: &[ResolvedModule]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for module in modules {
        for env_name in &module.when.env {
            if seen.insert(env_name.as_str()) {
                names.push(env_name.clone());
            }
        }
        for source in module.all_sources() {
            if let ResolvedSource::Env { name, .. } = source
                && seen.insert(name.as_str())
            {
                names.push(name.clone());
            }
        }
    }
    names
}

fn find_env_value<'a>(env_vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env_vars
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn validate_file_content(content: &str, regex: Option<&Regex>) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    apply_regex(trimmed, regex)
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

async fn resolve_command_source(
    cwd: PathBuf,
    path_env: Option<String>,
    args: Vec<String>,
    regex: Option<Regex>,
) -> Option<String> {
    let (program, cmd_args) = args.split_first()?;
    let mut command = tokio::process::Command::new(program);
    command.kill_on_drop(true).args(cmd_args).current_dir(cwd);
    if let Some(path_env) = path_env {
        command.env("PATH", path_env);
    }
    let output = command.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    apply_regex(trimmed, regex.as_ref())
}

async fn resolve_source_ref(
    cwd: &Path,
    env_vars: &[(String, String)],
    path_env: Option<&str>,
    source: &ResolvedSource,
) -> Option<String> {
    match source {
        ResolvedSource::Env { name, regex } => {
            find_env_value(env_vars, name).and_then(|value| apply_regex(value, regex.as_ref()))
        }
        ResolvedSource::File { path, regex } => {
            let path = cwd.join(path);
            let content = tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
                .await
                .ok()?
                .ok()?;
            validate_file_content(&content, regex.as_ref())
        }
        ResolvedSource::Command { args, regex } => {
            resolve_command_source(
                cwd.to_path_buf(),
                path_env.map(ToOwned::to_owned),
                args.clone(),
                regex.clone(),
            )
            .await
        }
    }
}
