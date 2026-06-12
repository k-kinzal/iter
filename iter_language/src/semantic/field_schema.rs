//! The generic string-map field intake shared by every per-kind lowerer.

use std::collections::BTreeMap;

use super::{Analyzer, MODE_HINT, TemplatePosition, closest, parse_priority};
use crate::ast::{AgentMode, OnErrorKeyword, PriorityKeyword, SecretExpr, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstField, CstIdent, CstValue};
use std::path::PathBuf;

impl Analyzer {
    pub(super) fn collect_fields(
        &mut self,
        body: Option<crate::parser::CstBlock>,
    ) -> BTreeMap<String, CstField> {
        let mut map = BTreeMap::new();
        if let Some(body) = body {
            for field in body.fields {
                if map.contains_key(&field.name.name) {
                    self.errors.push(Diagnostic::error(
                        field.name.span.clone(),
                        format!("duplicate field `{}` in block", field.name.name),
                    ));
                    continue;
                }
                map.insert(field.name.name.clone(), field);
            }
        }
        map
    }

    pub(super) fn require_kind(
        &mut self,
        kind: Option<CstIdent>,
        keyword_span: &Span,
        section: &str,
        valid: &[&str],
    ) -> Option<CstIdent> {
        if let Some(k) = kind {
            Some(k)
        } else {
            self.errors.push(
                Diagnostic::error(
                    keyword_span.clone(),
                    format!("`{section}` requires a kind name"),
                )
                .with_hint(format!("valid kinds: {}", valid.join(", "))),
            );
            None
        }
    }

    pub(super) fn reject_unknown_fields(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        valid: &[&str],
        context: &str,
    ) {
        let leftover: Vec<CstField> = std::mem::take(fields).into_values().collect();
        for f in leftover {
            let mut diag = Diagnostic::error(
                f.name.span.clone(),
                format!("unknown field `{}` in {context}", f.name.name),
            );
            if let Some(s) = closest(&f.name.name, valid) {
                diag = diag.with_hint(format!("did you mean `{s}`?"));
            } else if !valid.is_empty() {
                diag = diag.with_hint(format!("valid fields: {}", valid.join(", ")));
            }
            self.errors.push(diag);
        }
    }

    pub(super) fn take_required_string(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
        context: &str,
    ) -> Option<String> {
        self.take_required_string_with_hint(
            fields,
            name,
            kind_span,
            context,
            &format!("add `{name} = \"...\"`"),
        )
    }

    pub(super) fn take_required_string_with_hint(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
        context: &str,
        hint: &str,
    ) -> Option<String> {
        if let Some(field) = fields.remove(name) {
            match field.value {
                CstValue::String(s, _) => Some(s),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        format!("`{name}` must be a string"),
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), format!("{context} requires `{name}`"))
                    .with_hint(hint.to_string()),
            );
            None
        }
    }

    pub(super) fn take_optional_string(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<String> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::String(s, _) => Some(s),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a string"),
                ));
                None
            }
        }
    }

    pub(super) fn take_optional_int(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<i64> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Integer(n, _) => Some(n),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be an integer"),
                ));
                None
            }
        }
    }

    pub(super) fn take_optional_bool(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<bool> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Bool(b, _) => Some(b),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a boolean"),
                ));
                None
            }
        }
    }

    pub(super) fn take_optional_duration(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<i64> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Duration(secs, _) => Some(secs),
            CstValue::Integer(n, _) => Some(n), // accept bare integer seconds
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a duration (e.g. `5s`, `2m`, `1h`)"),
                ));
                None
            }
        }
    }

    pub(super) fn take_optional_string_list(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<Vec<String>> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::List(items, _) => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        CstValue::String(s, _) => out.push(s),
                        other => self.errors.push(Diagnostic::error(
                            other.span(),
                            format!("`{name}` list elements must be strings"),
                        )),
                    }
                }
                Some(out)
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a list of strings"),
                ));
                None
            }
        }
    }

    pub(super) fn take_required_bool_explicit(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
        context: &str,
        hint: &str,
    ) -> Option<bool> {
        if let Some(field) = fields.remove(name) {
            match field.value {
                CstValue::Bool(b, _) => Some(b),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        format!("`{name}` must be a boolean"),
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), format!("{context} requires `{name}`"))
                    .with_hint(hint.to_string()),
            );
            None
        }
    }

    pub(super) fn take_required_string_list_explicit(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
        context: &str,
        hint: &str,
    ) -> Option<Vec<String>> {
        if let Some(field) = fields.remove(name) {
            match field.value {
                CstValue::List(items, _) => {
                    let mut out = Vec::new();
                    for item in items {
                        match item {
                            CstValue::String(s, _) => out.push(s),
                            other => self.errors.push(Diagnostic::error(
                                other.span(),
                                format!("`{name}` list elements must be strings"),
                            )),
                        }
                    }
                    Some(out)
                }
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        format!("`{name}` must be a list of strings"),
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), format!("{context} requires `{name}`"))
                    .with_hint(hint.to_string()),
            );
            None
        }
    }

    /// Read an optional `SecretExpr`-typed field.
    ///
    /// Accepts either a string literal (becomes `SecretExpr::Literal`) or
    /// `env("VAR")` (becomes `SecretExpr::EnvVar`). Returns `None` if the
    /// field is absent.
    pub(super) fn take_optional_secret(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<SecretExpr> {
        let field = fields.remove(name)?;
        self.parse_secret_value(field.value, name)
    }

    /// Read a required `SecretExpr`-typed field. Emits a diagnostic and
    /// returns `None` when absent.
    pub(super) fn take_required_secret(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
        context: &str,
    ) -> Option<SecretExpr> {
        if let Some(field) = fields.remove(name) {
            self.parse_secret_value(field.value, name)
        } else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), format!("{context} requires `{name}`"))
                    .with_hint(format!("add `{name} = \"...\"` or `{name} = env(\"VAR\")`")),
            );
            None
        }
    }

    fn parse_secret_value(&mut self, value: CstValue, name: &str) -> Option<SecretExpr> {
        match value {
            CstValue::String(s, _) => Some(SecretExpr::Literal(s)),
            CstValue::Call {
                name: call_name,
                args,
                span,
            } if call_name == "env" => {
                if let Some(CstValue::String(s, _)) = args.into_iter().next() {
                    Some(SecretExpr::EnvVar(s))
                } else {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`env` requires a single string argument",
                    ));
                    None
                }
            }
            CstValue::Call {
                name: call_name,
                args,
                span,
            } if call_name == "file" => {
                if let Some(CstValue::String(s, _)) = args.into_iter().next() {
                    Some(SecretExpr::File(PathBuf::from(s)))
                } else {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`file` requires a single string argument",
                    ));
                    None
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!(
                        "`{name}` must be a string literal, `env(\"VAR\")`, or `file(\"path\")`"
                    ),
                ));
                None
            }
        }
    }

    /// Pop an optional `priority = <keyword>` field.
    pub(super) fn take_optional_priority(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<PriorityKeyword> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Ident(ident, span) => {
                if let Some(p) = parse_priority(&ident) {
                    Some(p)
                } else {
                    self.errors.push(
                        Diagnostic::error(span, format!("unknown priority `{ident}`"))
                            .with_hint("valid: low, normal, high, critical"),
                    );
                    None
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be one of low, normal, high, critical"),
                ));
                None
            }
        }
    }

    /// Pop an optional `on_error = <keyword>` field.
    pub(super) fn take_optional_on_error(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<OnErrorKeyword> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Ident(ident, span) => match ident.as_str() {
                "continue" => Some(OnErrorKeyword::Continue),
                "abort" => Some(OnErrorKeyword::Abort),
                "skip" => Some(OnErrorKeyword::Skip),
                other => {
                    self.errors.push(
                        Diagnostic::error(span, format!("unknown `on_error` value `{other}`"))
                            .with_hint("valid: continue, abort, skip"),
                    );
                    None
                }
            },
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be one of continue, abort, skip"),
                ));
                None
            }
        }
    }

    /// Pop an optional `u64` field. Accepts a non-negative integer.
    pub(super) fn take_optional_u64(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<u64> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Integer(n, span) => {
                if n < 0 {
                    self.errors.push(Diagnostic::error(
                        span,
                        format!("`{name}` must be a non-negative integer"),
                    ));
                    None
                } else {
                    Some(u64::try_from(n).expect("non-negative i64 fits in u64"))
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a non-negative integer"),
                ));
                None
            }
        }
    }

    /// Pop an optional `metadata { k = "v" ... }` block. Unlike
    /// [`Self::take_optional_string_string_block`], values preserve their
    /// `{{ ... }}` placeholders verbatim — they are template strings.
    pub(super) fn take_optional_metadata_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        position: TemplatePosition,
    ) -> Option<Vec<(String, String)>> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Block(block) => {
                let mut out = Vec::with_capacity(block.fields.len());
                for f in block.fields {
                    match f.value {
                        CstValue::String(s, span) => {
                            self.validate_template(&s, &span, position);
                            out.push((f.name.name, s));
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                other.span(),
                                format!("`{}.{}` must be a string", name, f.name.name),
                            ));
                        }
                    }
                }
                Some(out)
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a `{{ key = \"value\" ... }}` block"),
                ));
                None
            }
        }
    }

    /// Pop an optional string field whose value is a template, validating
    /// its `{{...}}` placeholders against `position`. Used for fields like
    /// the dead-letter `reason_template` that are rendered later by core.
    pub(super) fn take_optional_template_text(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        position: TemplatePosition,
    ) -> Option<String> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::String(s, span) => {
                self.validate_template(&s, &span, position);
                Some(s)
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a string"),
                ));
                None
            }
        }
    }

    /// Pop an optional `name { ... }` (or `name = { ... }`) sub-block,
    /// returning its inner field bag flattened the same way
    /// [`Self::collect_fields`] would.
    pub(super) fn take_optional_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<BTreeMap<String, CstField>> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::Block(b) => Some(self.collect_fields(Some(b))),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a block (`{name} {{ ... }}`)"),
                ));
                None
            }
        }
    }

    /// Pop an optional templated string field — accepts either a plain
    /// string literal or a single-argument `from_metadata("key")` call.
    pub(super) fn take_optional_templated_string(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<crate::ast::MetadataSource> {
        let field = fields.remove(name)?;
        match field.value {
            CstValue::String(s, _) => Some(crate::ast::MetadataSource::Literal(s)),
            CstValue::Call {
                name: call_name,
                args,
                span,
            } if call_name == "from_metadata" => {
                if let Some(CstValue::String(s, _)) = args.into_iter().next() {
                    Some(crate::ast::MetadataSource::FromMetadata(s))
                } else {
                    self.errors.push(Diagnostic::error(
                        span,
                        "`from_metadata` requires a single string argument",
                    ));
                    None
                }
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a string literal or `from_metadata(\"key\")`"),
                ));
                None
            }
        }
    }

    /// Pop an optional `name { k = "v"  ... }` block where every entry is
    /// a `string = string` pair. Used by structures like
    /// `message_attributes` whose values are all scalars.
    pub(super) fn take_optional_string_string_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<Vec<(String, String)>> {
        let mut inner = self.take_optional_block(fields, name)?;
        let mut out = Vec::with_capacity(inner.len());
        // Take in deterministic order; BTreeMap iterates sorted by key.
        let leftover: Vec<(String, CstField)> = std::mem::take(&mut inner).into_iter().collect();
        for (k, field) in leftover {
            match field.value {
                CstValue::String(v, _) => out.push((k, v)),
                other => self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}.{k}` must be a string"),
                )),
            }
        }
        Some(out)
    }

    /// Pop an optional `env { KEY = "value" ... }` block for agent
    /// declarations. Each key must match `[A-Z][A-Z0-9_]*` (POSIX
    /// environment variable naming) and each value must be a string
    /// literal. Duplicate keys are already rejected by
    /// [`collect_fields`].
    pub(super) fn take_optional_env_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> BTreeMap<String, String> {
        let Some(mut inner) = self.take_optional_block(fields, "env") else {
            return BTreeMap::new();
        };
        let mut out = BTreeMap::new();
        let leftover: Vec<(String, CstField)> = std::mem::take(&mut inner).into_iter().collect();
        for (k, field) in leftover {
            if !is_valid_env_name(&k) {
                self.errors.push(
                    Diagnostic::error(
                        field.name.span.clone(),
                        format!(
                            "invalid env name `{k}`: must match [A-Z][A-Z0-9_]*",
                        ),
                    )
                    .with_hint("environment variable names must be uppercase letters, digits, and underscores"),
                );
                continue;
            }
            match field.value {
                CstValue::String(v, _) => {
                    out.insert(k, v);
                }
                other => self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`env.{k}` must be a string"),
                )),
            }
        }
        out
    }

    pub(super) fn take_optional_string_kv_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> BTreeMap<String, String> {
        let Some(mut inner) = self.take_optional_block(fields, name) else {
            return BTreeMap::new();
        };
        let mut out = BTreeMap::new();
        let leftover: Vec<(String, CstField)> = std::mem::take(&mut inner).into_iter().collect();
        for (k, field) in leftover {
            match field.value {
                CstValue::String(v, _) => {
                    out.insert(k, v);
                }
                other => self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}.{k}` must be a string"),
                )),
            }
        }
        out
    }

    pub(super) fn take_required_agent_mode(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
        kind_name: &str,
    ) -> Option<AgentMode> {
        if let Some(field) = fields.remove("mode") {
            match field.value {
                CstValue::Ident(ident, span) => self.parse_agent_mode(&ident, span),
                other => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`mode` must be an identifier",
                    ));
                    None
                }
            }
        } else {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    format!("agent {kind_name} requires `mode`"),
                )
                .with_hint(MODE_HINT),
            );
            None
        }
    }
}

/// `true` when `name` matches `[A-Z][A-Z0-9_]*` — the POSIX convention
/// for environment variable names, excluding `_`-prefixed names which
/// are reserved for implementation use.
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}
