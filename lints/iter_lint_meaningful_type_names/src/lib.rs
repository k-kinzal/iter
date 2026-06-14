//! Require names to state domain content rather than generic container shape
//! or project-banned metaphor vocabulary.

#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_span;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use rustc_ast::ast::{Item, ItemKind};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_span::{FileNameDisplayPreference, Ident};

dylint_linting::declare_pre_expansion_lint! {
    /// ### What it does
    ///
    /// Denies custom type definitions whose names contain `Summary`,
    /// `Outcome`, or `Report`, or whose names end in `Result`; project-banned
    /// metaphor vocabulary in item identifiers; and dumping-ground source file
    /// names such as `inner.rs`, `util.rs`, or `config.rs`.
    ///
    /// ### Why is this bad?
    ///
    /// These names describe container shape or the standard `Result` monad,
    /// not the domain value the type holds.
    pub MEANINGFUL_TYPE_NAMES,
    Deny,
    "names must describe domain content, not generic container shape or project-banned vocabulary"
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemNameKind {
    Type,
    Function,
    Module,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordMatch {
    Exact,
    Prefix,
}

#[derive(Clone, Copy)]
struct VocabularyRule {
    word: &'static str,
    matching: WordMatch,
    kinds: &'static [ItemNameKind],
    say_instead: &'static str,
}

const ALL_ITEM_KINDS: &[ItemNameKind] = &[
    ItemNameKind::Type,
    ItemNameKind::Function,
    ItemNameKind::Module,
];
const TYPE_AND_MODULE: &[ItemNameKind] = &[ItemNameKind::Type, ItemNameKind::Module];
const TYPE_ONLY: &[ItemNameKind] = &[ItemNameKind::Type];

const BANNED_VOCABULARY: &[VocabularyRule] = &[
    VocabularyRule {
        word: "engine",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the actual noun doing the work: runner / queue / trigger / agent",
    },
    VocabularyRule {
        word: "bridge",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the real crossing: adapter / exporter / importer / conversion",
    },
    VocabularyRule {
        word: "assembly",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "the start: per-noun `*_from_def` fns / `runner_builder_from_plan`",
    },
    VocabularyRule {
        word: "assembl",
        matching: WordMatch::Prefix,
        kinds: ALL_ITEM_KINDS,
        say_instead: "the start: per-noun `*_from_def` fns / `runner_builder_from_plan`",
    },
    VocabularyRule {
        word: "construction",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "the start / build the named runtime value directly",
    },
    VocabularyRule {
        word: "construct",
        matching: WordMatch::Prefix,
        kinds: ALL_ITEM_KINDS,
        say_instead: "`new` constructors are fine; otherwise name the direct conversion, e.g. `*_from_def` or `*_from_plan`",
    },
    VocabularyRule {
        word: "factory",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "no Factory concept: each runtime type constructs itself",
    },
    VocabularyRule {
        word: "seam",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the one thing meant: the start / the observation contract / the telemetry split",
    },
    VocabularyRule {
        word: "substrate",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the actual layer or resource: workspace / queue / process / filesystem",
    },
    VocabularyRule {
        word: "machinery",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the mechanism directly: runner loop / trigger poller / queue driver",
    },
    VocabularyRule {
        word: "chassis",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the runtime holder directly: process / service / runner / workspace",
    },
    VocabularyRule {
        word: "plumbing",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the concrete path: IO / transport / registry / telemetry",
    },
    VocabularyRule {
        word: "sidecar",
        matching: WordMatch::Exact,
        kinds: ALL_ITEM_KINDS,
        say_instead: "name the actual companion process, trigger, or service",
    },
    VocabularyRule {
        word: "bootstrap",
        matching: WordMatch::Exact,
        kinds: TYPE_AND_MODULE,
        say_instead: "name the adoption token or start/adoption operation directly",
    },
    VocabularyRule {
        word: "rich",
        matching: WordMatch::Exact,
        kinds: TYPE_ONLY,
        say_instead: "name the extra content the type carries",
    },
];

const BANNED_FILE_NAMES: &[&str] = &[
    "inner.rs",
    "assembly.rs",
    "util.rs",
    "utils.rs",
    "helpers.rs",
    "common.rs",
    "misc.rs",
    "config.rs",
];

static REPORTED_BANNED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

impl EarlyLintPass for MeaningfulTypeNames {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        check_file_name(cx, item);

        let Some((ident, kind)) = item_ident(item) else {
            return;
        };
        check_vocabulary(cx, ident, kind);
        if kind != ItemNameKind::Type {
            return;
        }
        let name = ident.name.as_str();
        if name.contains("Summary") || name.contains("Outcome") || name.contains("Report") {
            cx.span_lint(MEANINGFUL_TYPE_NAMES, ident.span, |diag| {
                diag.primary_message(format!(
                    "`{name}` names a generic container, not the value's content"
                ));
                diag.help(
                    "rename it to the domain noun for what it holds. If you cannot, \
                     the type is usually a grab-bag (split it) or duplicates state \
                     emitted elsewhere (drop it).",
                );
            });
            return;
        }

        if name.ends_with("Result") {
            cx.span_lint(MEANINGFUL_TYPE_NAMES, ident.span, |diag| {
                diag.primary_message(format!(
                    "`{name}` ends in `Result` but is not the `Result` monad"
                ));
                diag.help(format!(
                    "rename to what it is so `Result<{name}, E>` does not read as a \
                     nested Result — e.g. `TransitionResult` -> `StatusTransition`."
                ));
            });
        }
    }
}

fn check_vocabulary(cx: &EarlyContext<'_>, ident: Ident, kind: ItemNameKind) {
    let name = ident.name.as_str();
    let words = identifier_words(name);
    for word in &words {
        let Some(rule) = BANNED_VOCABULARY
            .iter()
            .find(|rule| rule.kinds.contains(&kind) && rule.matches(word))
        else {
            continue;
        };

        cx.span_lint(MEANINGFUL_TYPE_NAMES, ident.span, |diag| {
            diag.primary_message(format!("`{name}` uses banned iter vocabulary `{word}`"));
            diag.help(format!("say instead: {}", rule.say_instead));
        });
        return;
    }
}

impl VocabularyRule {
    fn matches(self, word: &str) -> bool {
        match self.matching {
            WordMatch::Exact => word == self.word,
            WordMatch::Prefix => word.starts_with(self.word),
        }
    }
}

fn check_file_name(cx: &EarlyContext<'_>, item: &Item) {
    let file_name = cx
        .sess()
        .source_map()
        .span_to_filename(item.span)
        .display(FileNameDisplayPreference::Short)
        .to_string();

    if !BANNED_FILE_NAMES.contains(&file_name.as_str()) {
        return;
    }

    let file_path = cx
        .sess()
        .source_map()
        .span_to_filename(item.span)
        .display(FileNameDisplayPreference::Local)
        .to_string();
    if !mark_file_reported(&file_path) {
        return;
    }

    cx.span_lint(MEANINGFUL_TYPE_NAMES, item.span, |diag| {
        diag.primary_message(format!(
            "source file `{file_name}` uses a banned dumping-ground name"
        ));
        diag.help(format!(
            "name the file for the concept it defines; `{}` are dumping-ground names. \
             For on-disk layout, use `layout.rs`.",
            BANNED_FILE_NAMES.join("` / `")
        ));
    });
}

fn mark_file_reported(file_path: &str) -> bool {
    let normalized = Path::new(file_path).to_string_lossy().into_owned();
    let Ok(mut reported) = REPORTED_BANNED_FILES.lock() else {
        return true;
    };
    reported.insert(normalized)
}

fn item_ident(item: &Item) -> Option<(Ident, ItemNameKind)> {
    match &item.kind {
        ItemKind::Struct(ident, ..) | ItemKind::Enum(ident, ..) => {
            Some((*ident, ItemNameKind::Type))
        }
        ItemKind::TyAlias(alias) => Some((alias.ident, ItemNameKind::Type)),
        ItemKind::Fn(func) => Some((func.ident, ItemNameKind::Function)),
        ItemKind::Mod(_, ident, _) => Some((*ident, ItemNameKind::Module)),
        _ => None,
    }
}

fn identifier_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = name.chars().peekable();
    let mut previous_was_lowercase_or_digit = false;

    while let Some(ch) = chars.next() {
        if ch == '_' {
            push_word(&mut words, &mut current);
            previous_was_lowercase_or_digit = false;
            continue;
        }

        let next_is_lowercase = chars.peek().is_some_and(|next| next.is_ascii_lowercase());
        let starts_new_word = ch.is_ascii_uppercase()
            && !current.is_empty()
            && (previous_was_lowercase_or_digit || next_is_lowercase);

        if starts_new_word {
            push_word(&mut words, &mut current);
        }

        current.push(ch.to_ascii_lowercase());
        previous_was_lowercase_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }

    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}
