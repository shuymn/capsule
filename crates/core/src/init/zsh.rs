//! Zsh initialization script generation.
//!
//! Produces a zsh script that, when eval'd, sets up capsule prompt integration:
//! coproc relay, precmd/preexec hooks, and async update handling.

/// Generate the zsh initialization script.
///
/// The returned script is intended to be eval'd in the user's `.zshrc`:
///
/// ```zsh
/// eval "$(capsule init zsh)"
/// ```
#[must_use]
pub const fn generate() -> &'static str {
    include_str!("init.zsh")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_zsh_contains_capsule_functions() {
        let script = generate();
        assert!(
            script.contains("_capsule_precmd"),
            "missing _capsule_precmd"
        );
        assert!(
            script.contains("_capsule_preexec"),
            "missing _capsule_preexec"
        );
        assert!(script.contains("_capsule_init"), "missing _capsule_init");
        assert!(
            script.contains("_capsule_start_coproc"),
            "missing _capsule_start_coproc"
        );
        assert!(
            script.contains("_capsule_async_callback"),
            "missing _capsule_async_callback"
        );
    }

    #[test]
    fn test_init_zsh_contains_fallback_prompt() {
        let script = generate();
        assert!(script.contains("%~ %# "), "missing fallback prompt");
    }

    #[test]
    fn test_init_zsh_sets_prompt() {
        let script = generate();
        assert!(script.contains("PROMPT="), "script should set PROMPT");
    }

    #[test]
    fn test_init_zsh_adds_blank_line_before_prompt() {
        let script = generate();
        assert!(
            script.contains("# Match Starship's default add_newline behavior.\n    print"),
            "script should print a blank line before each prompt"
        );
    }

    #[test]
    fn test_init_zsh_registers_hooks() {
        let script = generate();
        assert!(
            script.contains("precmd_functions"),
            "should register precmd hook"
        );
        assert!(
            script.contains("preexec_functions"),
            "should register preexec hook"
        );
        assert!(
            script.contains("zshexit_functions"),
            "should register zshexit hook"
        );
    }

    #[test]
    fn test_init_zsh_calls_capsule_connect() {
        let script = generate();
        assert!(
            script.contains("capsule connect"),
            "coproc should run capsule connect"
        );
    }

    #[test]
    fn test_init_zsh_generates_session_id() {
        let script = generate();
        assert!(
            script.contains("_CAPSULE_SESSION_ID"),
            "should generate session ID"
        );
        assert!(script.contains("/dev/urandom"), "should use /dev/urandom");
    }

    #[test]
    fn test_init_zsh_calls_init_at_end() {
        let script = generate();
        assert!(
            script.trim_end().ends_with("_capsule_init"),
            "script should call _capsule_init as the last statement"
        );
    }

    #[test]
    fn test_init_zsh_no_global_setopt() {
        let script = generate();
        // Global setopt was removed — all options are now local to functions.
        assert!(
            !script.contains("setopt NO_CHECK_RUNNING_JOBS"),
            "should not set global shell options"
        );
    }
}
