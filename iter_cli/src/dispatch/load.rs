//! Iterfile loading and diagnostic rendering shared by every dispatch path.

use std::path::{Path, PathBuf};

use iter_language::{Diagnostic, Iterfile, parse};

use crate::output::{IntoExitCode, exit_codes};
use thiserror::Error;

/// Default path used when no `iterfile` argument is supplied.
pub(crate) const DEFAULT_ITERFILE: &str = "Iterfile";

/// Parsed Iterfile bundled with the resolved path so downstream code can
/// render path-qualified diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct LoadedIterfile {
    /// Validated AST.
    pub(crate) iterfile: Iterfile,
}

/// Errors produced by [`load_iterfile`].
#[derive(Debug, Error)]
pub(crate) enum LoadError {
    /// Reading the iterfile from disk failed.
    #[error("reading iterfile at {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The parser produced one or more error-severity diagnostics.
    #[error("{rendered}")]
    Parse {
        rendered: String,
        /// The leading diagnostic reports a compose-only named section
        /// (`queue main file { ... }`). `iter validate` keys its
        /// "use 'iter compose validate'" hint off this flag instead of
        /// re-parsing the file under a second grammar.
        compose_section: bool,
    },
}

impl IntoExitCode for LoadError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Read { .. } => exit_codes::USER_INPUT,
            Self::Parse { .. } => exit_codes::CONFIG,
        }
    }
}

/// Load `Iterfile` from `path` (or `./Iterfile` when `path` is `None`).
pub(crate) fn load_iterfile(path: Option<&Path>) -> Result<LoadedIterfile, LoadError> {
    let resolved = match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(DEFAULT_ITERFILE),
    };
    let source = std::fs::read_to_string(&resolved).map_err(|source| LoadError::Read {
        path: resolved.clone(),
        source,
    })?;
    let iterfile =
        parse(&source).map_err(|diags| render_diagnostics(&resolved, &source, &diags))?;
    Ok(LoadedIterfile { iterfile })
}

fn render_diagnostics(path: &Path, source: &str, diags: &[Diagnostic]) -> LoadError {
    if diags.is_empty() {
        return LoadError::Parse {
            rendered: format!(
                "iterfile {} failed to parse with no diagnostics",
                path.display()
            ),
            compose_section: false,
        };
    }
    let label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Iterfile");
    let mut rendered = String::new();
    for diag in diags {
        rendered.push_str(&diag.report(label, source));
        rendered.push('\n');
    }
    LoadError::Parse {
        rendered: rendered.trim_end().to_owned(),
        compose_section: diags.first().is_some_and(is_compose_section_diag),
    }
}

/// Whether `diag` reports a compose-only construct. Two shapes exist:
/// named sections (`queue main file { ... }`, `trigger nightly cron
/// { ... }`) read under the Iterfile grammar as a section with an
/// unexpected second identifier, and a `service` block is an unknown
/// Iterfile keyword. The check matches the analyzer's messages because
/// these constructs carry no dedicated diagnostic code — and a second
/// parse under the compose grammar would be a heavier way to get the
/// same answer.
fn is_compose_section_diag(diag: &Diagnostic) -> bool {
    if diag.message == "unknown top-level keyword `service`" {
        return true;
    }
    diag.message.starts_with("unexpected second identifier")
        && ["queue", "trigger"]
            .iter()
            .any(|kw| diag.message.ends_with(&format!("after `{kw}` section")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_a_valid_iterfile() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("Iterfile");
        std::fs::write(
            &path,
            r#"
queue memory
workspace local { base = "." }
agent claude {
    mode = print
    command = "claude"
}
runner {
    agent = claude
    workspace = local
    queue = memory
    continue_on_error = false
    behavior = wait
    prompt = "hello"
}
"#,
        )
        .expect("write");
        let loaded = load_iterfile(Some(&path)).expect("load");
        assert!(!loaded.iterfile.queues.is_empty());
        assert!(!loaded.iterfile.workspaces.is_empty());
        assert!(!loaded.iterfile.agents.is_empty());
        assert!(!loaded.iterfile.runners.is_empty());
        assert!(matches!(
            loaded.iterfile.runners.first().unwrap().node.prompt,
            iter_language::PromptExpr::Single(_)
        ));
    }

    #[test]
    fn missing_file_returns_error() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("does-not-exist");
        let err = load_iterfile(Some(&path)).expect_err("must fail");
        assert!(err.to_string().contains("reading iterfile"));
    }

    #[test]
    fn compose_named_section_sets_compose_section_flag() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("pipeline.iter");
        std::fs::write(&path, "queue main file { path = \"./q\" }\n").expect("write");
        let err = load_iterfile(Some(&path)).expect_err("must fail");
        let LoadError::Parse {
            compose_section, ..
        } = err
        else {
            panic!("expected parse error, got {err:?}");
        };
        assert!(compose_section, "named queue section must set the flag");
    }

    #[test]
    fn compose_service_section_sets_compose_section_flag() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("pipeline.iter");
        std::fs::write(&path, "service api { build = \"./Iterfile\" }\n").expect("write");
        let err = load_iterfile(Some(&path)).expect_err("must fail");
        let LoadError::Parse {
            compose_section, ..
        } = err
        else {
            panic!("expected parse error, got {err:?}");
        };
        assert!(compose_section, "leading service block must set the flag");
    }

    #[test]
    fn iterfile_errors_do_not_set_compose_section_flag() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("Iterfile");
        std::fs::write(&path, "queue redis { }\n").expect("write");
        let err = load_iterfile(Some(&path)).expect_err("must fail");
        let LoadError::Parse {
            compose_section, ..
        } = err
        else {
            panic!("expected parse error, got {err:?}");
        };
        assert!(!compose_section, "single-kind section must not set the flag");
    }

    #[test]
    fn syntax_errors_are_rendered_via_ariadne() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("Iterfile");
        std::fs::write(&path, "queue redis { }\n").expect("write");
        let err = load_iterfile(Some(&path)).expect_err("must fail");
        let msg = err.to_string();
        // Ariadne always emits a "Error:" or labelled span — accept either.
        assert!(
            msg.contains("Error") || msg.contains("error") || msg.contains("queue"),
            "expected diagnostic, got {msg:?}"
        );
    }
}
