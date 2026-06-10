//! Parser, AST, and semantic analyzer for the iter workflow definition language.
//!
//! This crate has ZERO dependencies on `iter_core` or any implementation crate.
//! It is a pure language crate: grammar specification, parsing, semantic analysis,
//! and a public AST for third-party tooling such as `tree-sitter-iter`, language
//! servers, editor plugins, and conformance test harnesses.
//!
//! # Stability
//!
//! The public API exposed from this crate (the [`Root`] AST, the
//! [`parse`] entry point, and the CST surface reached via [`parse_to_cst`])
//! is part of the iter language v1 contract. The grammar version is
//! [`GRAMMAR_VERSION`] and follows semantic versioning. The canonical
//! specification of the surface syntax lives at `grammar/iter.pest` inside
//! this crate; the pest file is the formal grammar that the hand-written
//! implementation is checked against via the crate's differential test
//! harness.
//!
//! # Example
//!
//! ```
//! use iter_language::parse;
//!
//! let source = r#"
//!     queue memory
//!     workspace clone {
//!         base = "."
//!         excludes = []
//!         preserve_mtime = true
//!         apply_back {
//!             mode = sync
//!         }
//!     }
//!     agent claude {
//!         mode = print
//!         command = "claude"
//!     }
//!     runner {
//!         agent = claude
//!         workspace = clone
//!         queue = memory
//!         continue_on_error = false
//!         behavior = wait
//!         prompt = "Do the thing"
//!     }
//! "#;
//! let root = parse(source).expect("valid source");
//! assert!(!root.queues.is_empty());
//! ```

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![forbid(unsafe_code)]

pub mod ast;
mod diagnostic;
mod lexer;
mod parser;
mod semantic;

pub use ast::{
    Action, AgentDecl, AgentMode, ApplyBackDecl, ArgDecl, CloneApplyBackMode, CmpOp, ComposeRoot,
    RouterStrategy,
    ComposeServiceOverride, ComposeTriggerOverride, DlqPolicyDecl, DlqTargetDecl, EventHandlerDecl,
    EventName, ExtractExpr, FilesSource, InlineService, IterationField, KafkaConfig, KafkaConsumer,
    KafkaProducer, KafkaSecurity, KinesisCheckpoint, KinesisConfig, KinesisConsumer,
    KinesisIdentity, KinesisProducer, KinesisShardListFilter, NamedCompose, NamedDef, NamedPrompt,
    NamedQueue, NamedService, NamedTrigger, OnErrorKeyword, PriorityKeyword, PromptArm, PromptDecl,
    PromptExpr, PromptGuard, PromptValue, PubSubConfig, PubSubCredentialKind, PubSubCredentials,
    PubSubInitialSeek, PubSubKeepalive, PubSubPublisher, PubSubSubscriber, QueueDecl, QueueRef,
    RetryPolicyDecl, Root, RunnerBehavior, RunnerDecl, SandboxNetworkDecl, SandboxPolicyDecl,
    SecretExpr, ServiceBusAuth, ServiceBusAuthKind, ServiceBusConfig, ServiceBusProxy,
    ServiceBusReceiver, ServiceBusSender, ServiceBusSession, ServiceSource, Span, Spanned,
    SqsConfig, SqsConsumer, SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity,
    SqsProducer, TelemetryDecl, TelemetryProtocol, TemplatedString, TriggerDecl, Value,
    WatchEventKind, WebhookRoute, WorkspaceDecl,
};
pub use diagnostic::{Diagnostic, Severity};
pub use parser::{
    RawAction, RawBlock, RawCmpOp, RawEventHandler, RawField, RawFile, RawGuard, RawIdent,
    RawPromptMatchArm, RawRoute, RawSection, RawValue,
};

/// Semantic version of the grammar implemented by this crate.
///
/// This version applies to three layers together:
///
/// 1. the surface syntax (as formally specified by `grammar/iter.pest`),
/// 2. the concrete syntax tree returned by [`parse_to_cst`] (the
///    [`RawFile`]/[`RawSection`]/[`RawValue`]/… hierarchy — their shape is
///    part of the contract, not just their presence), and
/// 3. the semantic AST returned by [`parse`] ([`Root`] and friends).
///
/// Any backwards-incompatible change (added or removed AST variants, changed
/// reserved keywords, removed kinds, removed CST fields) bumps the major
/// component. Adding a new optional field, a new kind that is parsed but not
/// required, or a new diagnostic message bumps the minor component. Bug
/// fixes and documentation changes bump the patch component.
pub const GRAMMAR_VERSION: &str = "4.1.0";

/// Parse the given source text into a validated [`Root`].
///
/// The pipeline is `lexer → parser → semantic analyzer`. All three stages
/// run with error recovery enabled, so a single call returns *every*
/// diagnostic discovered in the input rather than stopping at the first.
///
/// # Errors
///
/// Returns `Err(Vec<Diagnostic>)` containing one or more diagnostics if
/// either the syntactic or semantic analysis fails. The vector is never
/// empty when this function returns `Err`.
///
/// # Example
///
/// ```
/// use iter_language::parse;
///
/// let result = parse("queue memory\nworkspace local { base = \".\" }\n");
/// assert!(result.is_ok());
/// ```
pub fn parse(source: &str) -> Result<Root, Vec<Diagnostic>> {
    let (cst, mut diagnostics) = parse_to_cst(source);

    let root = match cst {
        Some(cst) => {
            let (lowered, sem_errors) = semantic::lower_and_check(cst);
            diagnostics.extend(sem_errors);
            lowered
        }
        None => None,
    };

    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics);
    }

    root.ok_or(diagnostics)
}

/// Parse the given source text into a concrete syntax tree without running
/// semantic analysis.
///
/// This is the syntactic-only entry point: it runs the lexer and parser and
/// returns whatever CST they produced together with every syntactic
/// diagnostic. Semantic validation (required fields, duplicate sections,
/// unknown event names, guard-expression checks, and so on) is deliberately
/// *not* performed. A vector of zero error-severity diagnostics means the
/// input is syntactically accepted.
///
/// The [`RawFile`] shape is part of the grammar contract — see
/// [`GRAMMAR_VERSION`].
///
/// The returned [`Option`] is `Some` whenever the parser produced a CST.
/// Today that is always the case; the `Option` is preserved to keep the
/// signature flexible for future hard-failure paths.
///
/// # Example
///
/// ```
/// use iter_language::parse_to_cst;
///
/// let (cst, diagnostics) = parse_to_cst("queue memory\n");
/// assert!(diagnostics.is_empty());
/// let cst = cst.expect("parser produced a CST");
/// assert_eq!(cst.sections.len(), 1);
/// ```
#[must_use]
pub fn parse_to_cst(source: &str) -> (Option<RawFile>, Vec<Diagnostic>) {
    let (tokens, lex_errors) = lexer::lex(source);
    let (cst, parse_errors) = parser::parse_tokens(&tokens, source.len());

    let mut diagnostics: Vec<Diagnostic> =
        Vec::with_capacity(lex_errors.len() + parse_errors.len());
    diagnostics.extend(lex_errors);
    diagnostics.extend(parse_errors);

    (cst, diagnostics)
}

/// Parse a `compose.iter` source file into a validated [`ComposeRoot`].
///
/// Shares the lexer + CST + per-kind builders with [`parse`], but interprets
/// each top-level section under compose semantics: the first identifier is
/// the section *name*, the optional second identifier is the kind. Four
/// section keywords are recognised: `queue`, `service`, `trigger`, and
/// `compose`. Any other section keyword (`prompt`, `runner`, top-level `on`)
/// is rejected at the compose layer because those sections only make sense
/// inside a service body.
///
/// # Errors
///
/// Returns `Err(Vec<Diagnostic>)` containing every syntactic and semantic
/// diagnostic surfaced by the pipeline. The vector is never empty when this
/// function returns `Err`.
pub fn parse_compose(source: &str) -> Result<ComposeRoot, Vec<Diagnostic>> {
    let (cst, mut diagnostics) = parse_to_cst(source);

    let root = match cst {
        Some(cst) => {
            let (lowered, sem_errors) = semantic::lower_compose_and_check(cst);
            diagnostics.extend(sem_errors);
            lowered
        }
        None => None,
    };

    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics);
    }

    root.ok_or(diagnostics)
}
