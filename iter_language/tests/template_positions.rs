//! End-to-end checks that template-position legality (R8) is enforced by
//! the analyzer, and that `{{arg.*}}` is cross-checked against declared
//! `arg`s. These drive the public `parse`/`parse_compose` entry points so
//! the position wiring — not just the position predicate — is covered.

use iter_language::{parse, parse_compose};

/// A complete Iterfile whose runner prompt is `prompt_body` and which is
/// preceded by `prologue` (e.g. `arg` declarations).
fn iterfile(prologue: &str, prompt_body: &str) -> String {
    format!(
        r#"{prologue}
queue memory
workspace clone {{
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back {{ mode = sync }}
}}
agent claude {{
  mode = print
  command = "claude"
}}
runner {{
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = false
  behavior = wait
  prompt = "{prompt_body}"
}}
"#
    )
}

#[test]
fn event_root_is_rejected_in_a_prompt() {
    let src = iterfile("", "{{event.action}}");
    assert!(
        parse(&src).is_err(),
        "`event` is not legal in a prompt body"
    );
}

#[test]
fn signal_metadata_iteration_today_are_legal_in_a_prompt() {
    let src = iterfile(
        "",
        "{{signal.id}} {{metadata.k}} {{iteration.count}} {{today}}",
    );
    assert!(
        parse(&src).is_ok(),
        "prompt accepts signal/metadata/iteration/today"
    );
}

#[test]
fn event_root_is_rejected_in_a_shell_action() {
    // The event handler lives inside the runner block; `{{event.*}}` is still
    // illegal in its shell action regardless of where the handler is declared.
    let src = r#"
queue memory
workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back { mode = sync }
}
agent claude {
  mode = print
  command = "claude"
}
runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = false
  behavior = wait
  prompt = "ok"
  on runner_finished { shell "echo {{event.action}}" }
}
"#;
    assert!(
        parse(src).is_err(),
        "`event` is not legal in an on-event shell action"
    );
}

#[test]
fn undeclared_arg_reference_is_an_error() {
    let src = iterfile("", "{{arg.missing}}");
    assert!(
        parse(&src).is_err(),
        "a template referencing an undeclared arg must be rejected"
    );
}

#[test]
fn declared_arg_reference_is_accepted() {
    let src = iterfile("arg worktree = \"w\"\n", "{{arg.worktree}}");
    assert!(parse(&src).is_ok(), "a declared arg may be referenced");
}

#[test]
fn declarable_and_referenceable_arg_names_share_a_grammar() {
    // `foo-bar` can never be a declared arg name (no `-`), so it must not
    // validate as a template reference either.
    let src = iterfile("", "{{arg.foo-bar}}");
    assert!(
        parse(&src).is_err(),
        "hyphenated arg references are rejected"
    );
}

const SQS_DLQ: &str = r#"queue sqs {
  queue_url = "https://sqs.us-east-1.amazonaws.com/123456789012/iter-signals"
  region = "us-east-1"
  dlq {
    kind = "native"
    reason_template = "REASON"
  }
}
workspace local { base = "." }
agent claude { mode = print  command = "claude" }
runner {
  agent = claude
  workspace = local
  queue = sqs
  continue_on_error = false
  behavior = wait
  prompt = "x"
}
"#;

#[test]
fn error_root_is_legal_in_a_dlq_reason_template() {
    let src = SQS_DLQ.replace("REASON", "{{error.kind}}");
    assert!(parse(&src).is_ok(), "DLQ reason templates may read `error`");
}

#[test]
fn non_error_roots_are_rejected_in_a_dlq_reason_template() {
    let src = SQS_DLQ.replace("REASON", "{{signal.id}}");
    assert!(
        parse(&src).is_err(),
        "DLQ reason templates accept only `error`, not `signal`"
    );
}

fn compose(trigger_block: &str) -> String {
    format!(
        r#"queue main file {{ path = "./.iter/queue" }}
{trigger_block}
"#
    )
}

#[test]
fn event_is_legal_in_a_webhook_subscription() {
    let src = compose(
        r#"trigger hook webhook {
    bind = "127.0.0.1:9000"
    path = "/hooks"
    target = main
    on "issues.*" when "{{event.action}} == 'opened'" {
        metadata { kind = "{{event.action}}" }
    }
}"#,
    );
    assert!(
        parse_compose(&src).is_ok(),
        "webhook subscriptions accept `event` in `when` and per-subscription metadata"
    );
}

#[test]
fn illegal_root_in_webhook_when_is_rejected_at_the_guard_span() {
    let src = compose(
        r#"trigger hook webhook {
    bind = "127.0.0.1:9000"
    path = "/hooks"
    target = main
    on "issues.*" when "{{error.kind}} == 'x'" {
        metadata { kind = "ok" }
    }
}"#,
    );
    let errors = parse_compose(&src).expect_err("`error` is illegal in a webhook `when` guard");
    let when_pos = src
        .find("{{error.kind}}")
        .expect("source contains the guard");
    // The diagnostic must be located *at* the `when` guard placeholder — not
    // ~20 bytes earlier at the `on` keyword, which is the bug that carrying
    // `when_span` fixes (the placeholder offset is measured from the guard
    // string's own span, modulo the 1-byte opening quote).
    assert!(
        errors.iter().any(|d| {
            d.span.start >= when_pos.saturating_sub(2)
                && d.span.start <= when_pos + "{{error.kind}}".len()
        }),
        "diagnostic should sit at the when-guard near byte {when_pos}; got {:?}",
        errors
            .iter()
            .map(|d| (d.span.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn event_is_rejected_in_trigger_base_metadata() {
    let src = compose(
        r#"trigger nightly cron {
    schedule = "0 0 * * *"
    target = main
    metadata { src = "{{event.action}}" }
}"#,
    );
    assert!(
        parse_compose(&src).is_err(),
        "trigger base metadata is stamped before any event, so `event` is illegal there"
    );
}
