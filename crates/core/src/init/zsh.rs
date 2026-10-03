//! Zsh integration using nonblocking pipes to a shell-owned worker.

/// Generate the zsh initialization script.
///
/// Evaluate the script once in `.zshrc`:
///
/// ```zsh
/// eval "$(capsule init zsh)"
/// ```
#[must_use]
pub const fn generate() -> &'static str {
    include_str!("init.zsh")
}
