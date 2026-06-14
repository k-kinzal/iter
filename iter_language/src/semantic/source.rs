//! `source { ... }` lowerer.

use std::collections::BTreeMap;

use super::Analyzer;
use crate::ast::{GitFastForward, GitLocator, SourceDef, SourceDerive, SourceDisposition, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstIdent, CstValue};

impl Analyzer {
    pub(super) fn lower_source(
        &mut self,
        kind: Option<CstIdent>,
        body: Option<CstBlock>,
        keyword_span: &Span,
    ) -> Option<SourceDef> {
        let kind = self.require_kind(kind, keyword_span, "source", &["directory", "git"])?;
        let mut fields = self.collect_fields(body);
        match kind.name.as_str() {
            "directory" => self.lower_source_directory(&mut fields, &kind.span),
            "git" => self.lower_source_git(&mut fields, &kind.span),
            other => {
                self.errors.push(
                    Diagnostic::error(kind.span, format!("unknown source kind `{other}`"))
                        .with_hint("valid kinds: directory, git"),
                );
                None
            }
        }
    }

    fn lower_source_directory(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SourceDef> {
        let path = self.take_required_string(fields, "path", kind_span, "source directory")?;
        let derive = self
            .take_optional_source_derive(fields)
            .unwrap_or(SourceDerive::Passthrough);
        let disposition = self.take_optional_source_disposition(fields);
        self.reject_unknown_fields(
            fields,
            &["path", "derive", "disposition"],
            "source directory",
        );

        if matches!(
            derive,
            SourceDerive::Worktree { .. } | SourceDerive::Clone { .. }
        ) {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "`worktree` and `clone` derive require `source git`",
                )
                .with_hint(
                    "use `derive = passthrough` or `derive = copy { ... }` for `source directory`",
                ),
            );
        }
        self.validate_derive_disposition(&derive, disposition.as_ref(), kind_span);

        Some(SourceDef::Directory {
            path,
            derive,
            disposition,
        })
    }

    fn lower_source_git(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SourceDef> {
        let url = self.take_optional_string(fields, "url");
        let path = self.take_optional_string(fields, "path");
        let locator = match (url, path) {
            (Some(_), Some(_)) => {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "source git requires exactly one of `url` or `path`, found both",
                ));
                None
            }
            (Some(url), None) => Some(GitLocator::Url(url)),
            (None, Some(path)) => Some(GitLocator::Path(path)),
            (None, None) => {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "source git requires exactly one of `url` or `path`",
                ));
                None
            }
        };
        let derive = self.take_optional_source_derive(fields).unwrap_or_else(|| {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), "source git requires `derive`")
                    .with_hint("add `derive = worktree` or `derive = clone`"),
            );
            SourceDerive::Worktree {
                ref_name: None,
                branch: None,
            }
        });
        let disposition = self.take_optional_source_disposition(fields);
        self.reject_unknown_fields(
            fields,
            &["url", "path", "derive", "disposition"],
            "source git",
        );

        if matches!(
            derive,
            SourceDerive::Passthrough | SourceDerive::Copy { .. }
        ) {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "`passthrough` and `copy` derive require `source directory`",
                )
                .with_hint("use `derive = worktree` or `derive = clone` for `source git`"),
            );
        }
        self.validate_derive_disposition(&derive, disposition.as_ref(), kind_span);

        Some(SourceDef::Git {
            locator: locator?,
            derive,
            disposition: disposition?,
        })
    }

    fn validate_derive_disposition(
        &mut self,
        derive: &SourceDerive,
        disposition: Option<&SourceDisposition>,
        span: &Span,
    ) {
        if matches!(derive, SourceDerive::Passthrough) && disposition.is_some() {
            self.errors.push(Diagnostic::error(
                span.clone(),
                "`disposition` is forbidden when `derive = passthrough`",
            ));
        }
        if !matches!(derive, SourceDerive::Passthrough) && disposition.is_none() {
            self.errors.push(Diagnostic::error(
                span.clone(),
                "`disposition` is required when `derive` creates a separate base",
            ));
        }
    }

    fn take_optional_source_derive(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<SourceDerive> {
        let field = fields.remove("derive")?;
        self.parse_source_derive_value(field.value)
    }

    fn parse_source_derive_value(&mut self, value: CstValue) -> Option<SourceDerive> {
        match value {
            CstValue::Ident(name, span) => self.parse_source_derive_kind(&name, span, None),
            CstValue::Block(block) => {
                let span = block.span.clone();
                let mut fields = self.collect_fields(Some(block));
                let kind = self.take_required_kind_field(&mut fields, &span, "derive")?;
                self.parse_source_derive_kind(&kind.0, kind.1, Some(fields))
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`derive` must be an identifier or tagged block",
                ));
                None
            }
        }
    }

    fn parse_source_derive_kind(
        &mut self,
        kind: &str,
        span: Span,
        fields: Option<BTreeMap<String, CstField>>,
    ) -> Option<SourceDerive> {
        let mut fields = fields.unwrap_or_default();
        match kind {
            "passthrough" => {
                self.reject_unknown_fields(&mut fields, &["kind"], "source derive passthrough");
                Some(SourceDerive::Passthrough)
            }
            "copy" => {
                let excludes = self
                    .take_optional_string_list(&mut fields, "excludes")
                    .unwrap_or_default();
                let preserve_mtime = self
                    .take_optional_bool(&mut fields, "preserve_mtime")
                    .unwrap_or(true);
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "excludes", "preserve_mtime"],
                    "source derive copy",
                );
                Some(SourceDerive::Copy {
                    excludes,
                    preserve_mtime,
                })
            }
            "worktree" => {
                let ref_name = self.take_optional_string(&mut fields, "ref");
                let branch = self.take_optional_string(&mut fields, "branch");
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "ref", "branch"],
                    "source derive worktree",
                );
                Some(SourceDerive::Worktree { ref_name, branch })
            }
            "clone" => {
                let ref_name = self.take_optional_string(&mut fields, "ref");
                let branch = self.take_optional_string(&mut fields, "branch");
                let depth = self.take_optional_u64(&mut fields, "depth");
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "ref", "branch", "depth"],
                    "source derive clone",
                );
                Some(SourceDerive::Clone {
                    ref_name,
                    branch,
                    depth,
                })
            }
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown source derive `{other}`"))
                        .with_hint("valid derives: passthrough, copy, worktree, clone"),
                );
                None
            }
        }
    }

    fn take_optional_source_disposition(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<SourceDisposition> {
        let field = fields.remove("disposition")?;
        self.parse_source_disposition_value(field.value)
    }

    fn parse_source_disposition_value(&mut self, value: CstValue) -> Option<SourceDisposition> {
        match value {
            CstValue::Ident(name, span) => self.parse_source_disposition_kind(&name, span, None),
            CstValue::Block(block) => {
                let span = block.span.clone();
                let mut fields = self.collect_fields(Some(block));
                let kind = self.take_required_kind_field(&mut fields, &span, "disposition")?;
                self.parse_source_disposition_kind(&kind.0, kind.1, Some(fields))
            }
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`disposition` must be an identifier or tagged block",
                ));
                None
            }
        }
    }

    fn parse_source_disposition_kind(
        &mut self,
        kind: &str,
        span: Span,
        fields: Option<BTreeMap<String, CstField>>,
    ) -> Option<SourceDisposition> {
        let mut fields = fields.unwrap_or_default();
        match kind {
            "discard" => {
                self.reject_unknown_fields(&mut fields, &["kind"], "source disposition discard");
                Some(SourceDisposition::Discard)
            }
            "merge" => {
                let excludes = self
                    .take_optional_string_list(&mut fields, "excludes")
                    .unwrap_or_default();
                let includes = self
                    .take_optional_string_list(&mut fields, "includes")
                    .unwrap_or_default();
                let into = self.take_optional_string(&mut fields, "into");
                let ff = self.take_optional_git_ff(&mut fields);
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "excludes", "includes", "into", "ff"],
                    "source disposition merge",
                );
                Some(SourceDisposition::Merge {
                    excludes,
                    includes,
                    into,
                    ff,
                })
            }
            "sync" => {
                let excludes = self
                    .take_optional_string_list(&mut fields, "excludes")
                    .unwrap_or_default();
                let includes = self
                    .take_optional_string_list(&mut fields, "includes")
                    .unwrap_or_default();
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "excludes", "includes"],
                    "source disposition sync",
                );
                Some(SourceDisposition::Sync { excludes, includes })
            }
            "defer" => {
                let promote = self.take_required_promote_disposition(&mut fields, &span)?;
                if matches!(promote, SourceDisposition::Defer { .. }) {
                    self.errors.push(Diagnostic::error(
                        span.clone(),
                        "`defer.promote` cannot itself be `defer`",
                    ));
                }
                self.reject_unknown_fields(
                    &mut fields,
                    &["kind", "promote"],
                    "source disposition defer",
                );
                Some(SourceDisposition::Defer {
                    promote: Box::new(promote),
                })
            }
            other => {
                self.errors.push(
                    Diagnostic::error(span, format!("unknown source disposition `{other}`"))
                        .with_hint("valid dispositions: discard, merge, sync, defer"),
                );
                None
            }
        }
    }

    fn take_required_kind_field(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        span: &Span,
        context: &str,
    ) -> Option<(String, Span)> {
        let Some(field) = fields.remove("kind") else {
            self.errors.push(Diagnostic::error(
                span.clone(),
                format!("`{context}` block requires `kind`"),
            ));
            return None;
        };
        match field.value {
            CstValue::Ident(name, span) => Some((name, span)),
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{context}.kind` must be an identifier"),
                ));
                None
            }
        }
    }

    fn take_required_promote_disposition(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        span: &Span,
    ) -> Option<SourceDisposition> {
        let Some(field) = fields.remove("promote") else {
            self.errors.push(Diagnostic::error(
                span.clone(),
                "`disposition = defer` requires `promote`",
            ));
            return None;
        };
        self.parse_source_disposition_value(field.value)
    }

    fn take_optional_git_ff(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<GitFastForward> {
        let field = fields.remove("ff")?;
        match field.value {
            CstValue::Ident(name, span) => match name.as_str() {
                "allow" => Some(GitFastForward::Allow),
                "only" => Some(GitFastForward::Only),
                "no" => Some(GitFastForward::No),
                other => {
                    self.errors.push(
                        Diagnostic::error(
                            span,
                            format!("unknown git fast-forward policy `{other}`"),
                        )
                        .with_hint("valid: allow, only, no"),
                    );
                    None
                }
            },
            other => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`ff` must be one of allow, only, no",
                ));
                None
            }
        }
    }
}
