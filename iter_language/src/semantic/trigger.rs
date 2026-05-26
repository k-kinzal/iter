//! `trigger { ... }` lowerer (all built-in kinds plus the external fallback).

use std::collections::BTreeMap;

use super::Analyzer;
use super::value::value_from_raw_pure;
use crate::ast::{ExtractExpr, FilesSource, PriorityKeyword, Span, TriggerDecl, WatchEventKind};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawField, RawIdent, RawValue};

/// Common trigger fields shared by every built-in kind except `loop` and
/// `external`.
struct CommonTriggerFields {
    base_metadata: Vec<(String, String)>,
    priority: Option<PriorityKeyword>,
    max_signals: Option<u64>,
}

impl Analyzer {
    pub(super) fn lower_trigger(
        &mut self,
        kind: Option<RawIdent>,
        body: Option<RawBlock>,
        keyword_span: &Span,
    ) -> Option<TriggerDecl> {
        let kind = self.require_kind(
            kind,
            keyword_span,
            "trigger",
            &["loop", "cron", "watch", "files", "command", "webhook"],
        )?;
        let body_block = body;
        let mut fields = self.collect_fields(body_block.clone());
        let decl = match kind.name.as_str() {
            "loop" => self.lower_trigger_loop(&mut fields),
            "cron" => self.lower_trigger_cron(&mut fields, &kind.span)?,
            "watch" => self.lower_trigger_watch(&mut fields, &kind.span)?,
            "files" => self.lower_trigger_files(&mut fields, &kind.span)?,
            "command" => self.lower_trigger_command(&mut fields, &kind.span)?,
            "webhook" => {
                self.lower_trigger_webhook(&mut fields, body_block.as_ref(), &kind.span)?
            }
            other => Self::lower_trigger_external(other, fields),
        };
        Some(decl)
    }

    fn lower_trigger_loop(&mut self, fields: &mut BTreeMap<String, RawField>) -> TriggerDecl {
        let max_iteration = self.take_optional_int(fields, "max_iteration");
        let delay_secs = self.take_optional_duration(fields, "delay");
        self.reject_unknown_fields(fields, &["max_iteration", "delay"], "trigger loop");
        TriggerDecl::Loop {
            max_iteration,
            delay_secs,
        }
    }

    fn lower_trigger_cron(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<TriggerDecl> {
        let schedule = self.take_required_string(fields, "schedule", kind_span, "trigger cron")?;
        let timezone = self.take_optional_string(fields, "timezone");
        let at_startup = self
            .take_optional_bool(fields, "at_startup")
            .unwrap_or(false);
        let catch_up_secs = self.take_optional_duration(fields, "catch_up");
        let jitter_secs = self.take_optional_duration(fields, "jitter");
        let common = self.take_common_trigger_fields(fields);
        self.reject_unknown_fields(
            fields,
            &[
                "schedule",
                "timezone",
                "at_startup",
                "catch_up",
                "jitter",
                "metadata",
                "priority",
                "max_signals",
            ],
            "trigger cron",
        );
        Some(TriggerDecl::Cron {
            schedule,
            timezone,
            at_startup,
            catch_up_secs,
            jitter_secs,
            base_metadata: common.base_metadata,
            priority: common.priority,
            max_signals: common.max_signals,
        })
    }

    fn lower_trigger_watch(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<TriggerDecl> {
        let dir = self.take_required_string(fields, "dir", kind_span, "trigger watch")?;
        let include = self
            .take_optional_string_list(fields, "include")
            .unwrap_or_default();
        let exclude = self
            .take_optional_string_list(fields, "exclude")
            .unwrap_or_default();
        let kinds = self.take_watch_kinds(fields);
        let per_file = self.take_optional_bool(fields, "per_file").unwrap_or(false);
        let interval_secs = self.take_optional_duration(fields, "interval");
        let cooldown_secs = self.take_optional_duration(fields, "cooldown");
        if interval_secs.is_some() && cooldown_secs.is_some() {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "trigger watch: `interval` and `cooldown` are mutually exclusive",
                )
                .with_hint("`cooldown` is a deprecated alias for `interval`; use `interval` only"),
            );
        }
        let interval_secs = interval_secs.or(cooldown_secs);
        if let Some(secs) = interval_secs {
            if secs <= 0 {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "trigger watch: `interval` must be a positive duration",
                ));
            }
        }
        let common = self.take_common_trigger_fields(fields);
        self.reject_unknown_fields(
            fields,
            &[
                "dir",
                "include",
                "exclude",
                "kinds",
                "per_file",
                "interval",
                "cooldown",
                "metadata",
                "priority",
                "max_signals",
            ],
            "trigger watch",
        );
        Some(TriggerDecl::Watch {
            dir,
            include,
            exclude,
            kinds,
            per_file,
            interval_secs,
            base_metadata: common.base_metadata,
            priority: common.priority,
            max_signals: common.max_signals,
        })
    }

    fn lower_trigger_files(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<TriggerDecl> {
        let sources = self.lower_files_sources(fields, kind_span)?;
        let no_exit_on_eof = self
            .take_optional_bool(fields, "no_exit_on_eof")
            .unwrap_or(false);
        let common = self.take_common_trigger_fields(fields);
        self.reject_unknown_fields(
            fields,
            &[
                "from",
                "path",
                "no_exit_on_eof",
                "metadata",
                "priority",
                "max_signals",
            ],
            "trigger files",
        );
        Some(TriggerDecl::Files {
            sources,
            no_exit_on_eof,
            base_metadata: common.base_metadata,
            priority: common.priority,
            max_signals: common.max_signals,
        })
    }

    fn lower_trigger_command(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<TriggerDecl> {
        let run = self.take_required_string(fields, "run", kind_span, "trigger command")?;
        let shell = self.take_optional_string(fields, "shell");
        let extract = self.take_extract_expr(fields);
        let poll_secs = self.take_optional_duration(fields, "poll");
        let dedupe = self.take_optional_bool(fields, "dedupe").unwrap_or(false);
        let on_error = self.take_optional_on_error(fields, "on_error");
        let common = self.take_common_trigger_fields(fields);
        self.reject_unknown_fields(
            fields,
            &[
                "run",
                "shell",
                "extract",
                "poll",
                "dedupe",
                "on_error",
                "metadata",
                "priority",
                "max_signals",
            ],
            "trigger command",
        );
        Some(TriggerDecl::Command {
            run,
            shell,
            extract,
            poll_secs,
            dedupe,
            on_error,
            base_metadata: common.base_metadata,
            priority: common.priority,
            max_signals: common.max_signals,
        })
    }

    fn take_extract_expr(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<ExtractExpr> {
        let field = fields.remove("extract")?;
        match field.value {
            RawValue::Call { name, args, span } => match name.as_str() {
                "regex" => {
                    if let Some(RawValue::String(s, _)) = args.into_iter().next() {
                        Some(ExtractExpr::Regex(s))
                    } else {
                        self.errors.push(Diagnostic::error(
                            span,
                            "`regex` requires a single string argument",
                        ));
                        None
                    }
                }
                other => {
                    self.errors.push(
                        Diagnostic::error(span, format!("unknown extractor `{other}`"))
                            .with_hint("supported: regex(\"...\")"),
                    );
                    None
                }
            },
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`extract` must be `regex(\"...\")`",
                ));
                None
            }
        }
    }

    fn lower_trigger_webhook(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        body_block: Option<&RawBlock>,
        kind_span: &Span,
    ) -> Option<TriggerDecl> {
        let host = self.take_optional_string(fields, "host");
        let bind = self.take_optional_string(fields, "bind");
        let port = self.take_optional_int(fields, "port");
        let path = self.take_required_string(fields, "path", kind_span, "trigger webhook")?;
        let secret = self.take_optional_secret(fields, "secret");
        let common = self.take_common_trigger_fields(fields);
        let routes = body_block
            .map(|b| self.lower_webhook_routes(&b.routes))
            .unwrap_or_default();
        self.reject_unknown_fields(
            fields,
            &[
                "host",
                "port",
                "bind",
                "path",
                "secret",
                "metadata",
                "priority",
                "max_signals",
            ],
            "trigger webhook",
        );

        if bind.is_some() && (host.is_some() || port.is_some()) {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "trigger webhook: `bind` is mutually exclusive with `host`/`port`",
                )
                .with_hint("use either `bind = \"0.0.0.0:8080\"` or `host`/`port`"),
            );
        }
        if bind.is_none() && port.is_none() {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "trigger webhook requires `port` (or `bind`)",
                )
                .with_hint("add `port = 8080` or `bind = \"0.0.0.0:8080\"`"),
            );
            return None;
        }

        Some(TriggerDecl::Webhook {
            host,
            port,
            bind,
            path,
            secret,
            routes,
            base_metadata: common.base_metadata,
            priority: common.priority,
            max_signals: common.max_signals,
        })
    }

    fn lower_trigger_external(name: &str, fields: BTreeMap<String, RawField>) -> TriggerDecl {
        let mut config = BTreeMap::new();
        for field in fields.into_values() {
            config.insert(field.name.name.clone(), value_from_raw_pure(field.value));
        }
        TriggerDecl::External {
            name: name.to_string(),
            config,
        }
    }

    fn take_common_trigger_fields(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> CommonTriggerFields {
        let base_metadata = self
            .take_optional_metadata_block(fields, "metadata")
            .unwrap_or_default();
        let priority = self.take_optional_priority(fields, "priority");
        let max_signals = self.take_optional_u64(fields, "max_signals");
        CommonTriggerFields {
            base_metadata,
            priority,
            max_signals,
        }
    }

    fn take_watch_kinds(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Vec<WatchEventKind> {
        let Some(field) = fields.remove("kinds") else {
            return Vec::new();
        };
        let (items, list_span) = match field.value {
            RawValue::List(items, span) => {
                if items.is_empty() {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`kinds` list must not be empty; omit the field to allow all event kinds",
                    ));
                    return Vec::new();
                }
                (items, span)
            }
            other => {
                self.errors.push(
                    Diagnostic::error(
                        other.span(),
                        "`kinds` must be a list of strings",
                    )
                    .with_hint("e.g. kinds = [\"created\", \"modified\"]"),
                );
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(items.len());
        let mut seen = std::collections::HashSet::new();
        let mut had_error = false;
        for item in items {
            match item {
                RawValue::String(s, span) => match s.as_str() {
                    "created" | "modified" | "removed" => {
                        let kind = match s.as_str() {
                            "created" => WatchEventKind::Created,
                            "modified" => WatchEventKind::Modified,
                            "removed" => WatchEventKind::Removed,
                            _ => unreachable!(),
                        };
                        if seen.insert(kind) {
                            out.push(kind);
                        } else {
                            self.errors.push(Diagnostic::warning(
                                span,
                                format!("duplicate kind `{s}` in `kinds` list"),
                            ));
                        }
                    }
                    _ => {
                        had_error = true;
                        self.errors.push(
                            Diagnostic::error(
                                span,
                                format!(
                                    "unknown watch event kind `{s}`"
                                ),
                            )
                            .with_hint("valid values: \"created\", \"modified\", \"removed\""),
                        );
                    }
                },
                other => {
                    had_error = true;
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`kinds` list elements must be strings",
                    ));
                }
            }
        }
        if seen.len() == 3 && !had_error {
            self.errors.push(
                Diagnostic::warning(
                    list_span,
                    "trigger watch: listing all three kinds is equivalent to omitting `kinds`",
                ),
            );
        }
        out
    }

    fn lower_files_sources(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<Vec<FilesSource>> {
        if let Some(field) = fields.remove("from") {
            match field.value {
                RawValue::Ident(name, span) if name == "stdin" => {
                    let _ = span;
                    Some(vec![FilesSource::Stdin])
                }
                RawValue::String(s, span) => {
                    if let Some(src) = parse_files_source_string(&s) {
                        Some(vec![src])
                    } else {
                        self.errors.push(Diagnostic::error(
                            span,
                            "`from` string must be `path:<file>` or a bare path",
                        ));
                        None
                    }
                }
                RawValue::List(items, list_span) => {
                    if items.is_empty() {
                        self.errors.push(Diagnostic::error(
                            list_span,
                            "`from` list must contain at least one source",
                        ));
                        return None;
                    }
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        match item {
                            RawValue::String(s, span) => match parse_files_source_string(&s) {
                                Some(src) => out.push(src),
                                None => {
                                    self.errors.push(Diagnostic::error(
                                        span,
                                        "`from` list elements must be `path:<file>` or a bare path",
                                    ));
                                }
                            },
                            RawValue::Ident(name, span) if name == "stdin" => {
                                self.errors.push(
                                    Diagnostic::error(
                                        span,
                                        "`stdin` is not allowed inside a `from` list",
                                    )
                                    .with_hint(
                                        "use `from = stdin` (scalar) or list path sources only",
                                    ),
                                );
                            }
                            other => {
                                self.errors.push(Diagnostic::error(
                                    other.span(),
                                    "`from` list elements must be strings",
                                ));
                            }
                        }
                    }
                    if out.is_empty() { None } else { Some(out) }
                }
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`from` must be `stdin`, a string path, or a list of path strings",
                    ));
                    None
                }
            }
        } else if let Some(field) = fields.remove("path") {
            match field.value {
                RawValue::String(s, _) => Some(vec![FilesSource::Path(s)]),
                other => {
                    self.errors
                        .push(Diagnostic::error(other.span(), "`path` must be a string"));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), "trigger files requires `from` or `path`")
                    .with_hint("e.g. `from = stdin` or `path = \"./list.txt\"`"),
            );
            None
        }
    }
}

fn parse_files_source_string(s: &str) -> Option<FilesSource> {
    if let Some(rest) = s.strip_prefix("path:") {
        if rest.is_empty() {
            None
        } else {
            Some(FilesSource::Path(rest.to_owned()))
        }
    } else if s.is_empty() {
        None
    } else {
        // A bare string — treat it as a path.
        Some(FilesSource::Path(s.to_owned()))
    }
}
