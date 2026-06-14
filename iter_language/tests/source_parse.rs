use iter_language::{SourceDef, SourceDerive, parse};

fn parse_errs(source: &str) -> Vec<String> {
    parse(source)
        .expect_err("source should be invalid")
        .into_iter()
        .map(|d| d.message)
        .collect()
}

#[test]
fn parses_directory_and_git_sources() {
    let root = parse(
        r#"
source directory as snap {
  path = "/repo"
  derive = copy { excludes = ["target"] preserve_mtime = false }
  disposition = merge { excludes = ["*.tmp"] includes = ["src/**"] }
}
source git as wt {
  path = "/repo"
  derive = worktree { ref = "HEAD" branch = "iter/test" }
  disposition = merge { into = "main" ff = only }
}
source git as cloned {
  url = "https://example.invalid/repo.git"
  derive = clone { ref = "main" depth = 1 }
  disposition = discard
}
workspace clone as dev {
  source = snap
  excludes = []
  preserve_mtime = true
  apply_back { mode = merge }
}
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = dev
  continue_on_error = false
  behavior = loop
  prompt = "noop"
}
"#,
    )
    .expect("valid source syntax");

    assert_eq!(root.sources.len(), 3);
    assert!(matches!(
        root.sources[0].node.decl,
        SourceDef::Directory {
            derive: SourceDerive::Copy { .. },
            ..
        }
    ));
}

#[test]
fn source_path_sugar_on_workspace_is_valid() {
    let root = parse(
        r#"
workspace local { source = "/repo" }
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = loop
  prompt = "noop"
}
"#,
    )
    .expect("valid source path sugar");
    assert!(root.sources.is_empty());
}

#[test]
fn rejects_required_source_errors() {
    let cases = [
        (
            r#"source directory { path = "/repo" derive = passthrough disposition = discard }"#,
            "`disposition` is forbidden when `derive = passthrough`",
        ),
        (
            r#"source directory { path = "/repo" derive = copy }"#,
            "`disposition` is required when `derive` creates a separate base",
        ),
        (
            r#"source directory { path = "/repo" derive = worktree disposition = discard }"#,
            "`worktree` and `clone` derive require `source git`",
        ),
        (
            r#"source git { url = "u" path = "/repo" derive = clone disposition = discard }"#,
            "source git requires exactly one of `url` or `path`, found both",
        ),
        (
            r"source git { derive = clone disposition = discard }",
            "source git requires exactly one of `url` or `path`",
        ),
        (
            r#"source directory { path = "/repo" derive = copy disposition = defer { promote = defer { promote = discard } } }"#,
            "`defer.promote` cannot itself be `defer`",
        ),
        (
            r"workspace local { source = missing }",
            "workspace references source `missing` which is not defined",
        ),
        (
            r#"workspace local { base = "/repo" source = other }"#,
            "workspace local cannot set both `base` and `source`",
        ),
    ];

    for (source, expected) in cases {
        let messages = parse_errs(source);
        assert!(
            messages.iter().any(|m| m.contains(expected)),
            "expected {expected:?} in {messages:?}",
        );
    }
}
