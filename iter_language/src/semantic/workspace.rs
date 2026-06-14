//! `workspace { ... }` lowerer (including sandbox policy / network / apply-back
//! sub-helpers).

use std::collections::BTreeMap;

use super::{
    APPLY_BACK_DISCARD_FILTER_HINT, APPLY_BACK_HINT, APPLY_BACK_MODE_HINT, Analyzer, EXCLUDES_HINT,
    NETWORK_HINT, POLICY_HINT, PRESERVE_MTIME_HINT,
};
use crate::ast::{
    ApplyBackDef, CloneApplyBackMode, SandboxNetworkDef, SandboxPolicyDef, Span, WorkspaceDef,
    WorkspaceSourceRef,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstIdent, CstValue};

impl Analyzer {
    pub(super) fn lower_workspace(
        &mut self,
        kind: Option<CstIdent>,
        body: Option<CstBlock>,
        keyword_span: &Span,
    ) -> Option<WorkspaceDef> {
        let kind = self.require_kind(
            kind,
            keyword_span,
            "workspace",
            &["local", "clone", "sandbox"],
        )?;
        let mut fields = self.collect_fields(body);
        match kind.name.as_str() {
            "local" => self.lower_workspace_local(&mut fields, &kind.span),
            "clone" => self.lower_workspace_clone(&mut fields, &kind.span),
            "sandbox" => self.lower_workspace_sandbox(&mut fields, &kind.span),
            other => {
                self.errors.push(
                    Diagnostic::error(kind.span, format!("unknown workspace kind `{other}`"))
                        .with_hint("valid kinds: local, clone, sandbox"),
                );
                None
            }
        }
    }

    fn lower_workspace_local(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<WorkspaceDef> {
        let (base, source) =
            self.take_workspace_base_or_source(fields, kind_span, "workspace local")?;
        self.reject_unknown_fields(fields, &["base", "source"], "workspace local");
        Some(WorkspaceDef::Local { base, source })
    }

    fn lower_workspace_clone(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<WorkspaceDef> {
        let (base, source) =
            self.take_workspace_base_or_source(fields, kind_span, "workspace clone")?;
        let remote = self.take_optional_string(fields, "remote");
        let excludes = self.take_required_string_list_explicit(
            fields,
            "excludes",
            kind_span,
            "workspace clone",
            EXCLUDES_HINT,
        );
        let includes = self
            .take_optional_string_list(fields, "includes")
            .unwrap_or_default();
        let preserve_mtime = self.take_required_bool_explicit(
            fields,
            "preserve_mtime",
            kind_span,
            "workspace clone",
            PRESERVE_MTIME_HINT,
        );
        let apply_back = self.take_required_apply_back_block(fields, kind_span, "workspace clone");
        self.reject_unknown_fields(
            fields,
            &[
                "base",
                "source",
                "remote",
                "excludes",
                "includes",
                "preserve_mtime",
                "apply_back",
            ],
            "workspace clone",
        );
        Some(WorkspaceDef::Clone {
            base,
            source,
            remote,
            excludes: excludes?,
            includes,
            preserve_mtime: preserve_mtime?,
            apply_back: apply_back?,
        })
    }

    fn lower_workspace_sandbox(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<WorkspaceDef> {
        let (base, source) =
            self.take_workspace_base_or_source(fields, kind_span, "workspace sandbox")?;
        let excludes = self.take_required_string_list_explicit(
            fields,
            "excludes",
            kind_span,
            "workspace sandbox",
            EXCLUDES_HINT,
        );
        let includes = self
            .take_optional_string_list(fields, "includes")
            .unwrap_or_default();
        let preserve_mtime = self.take_required_bool_explicit(
            fields,
            "preserve_mtime",
            kind_span,
            "workspace sandbox",
            PRESERVE_MTIME_HINT,
        );
        let apply_back =
            self.take_required_apply_back_block(fields, kind_span, "workspace sandbox");
        let policy = self.take_required_sandbox_policy(fields, kind_span);
        self.reject_unknown_fields(
            fields,
            &[
                "base",
                "source",
                "excludes",
                "includes",
                "preserve_mtime",
                "apply_back",
                "policy",
            ],
            "workspace sandbox",
        );
        Some(WorkspaceDef::Sandbox {
            base,
            source,
            excludes: excludes?,
            includes,
            preserve_mtime: preserve_mtime?,
            apply_back: apply_back?,
            policy: policy?,
        })
    }

    fn take_workspace_base_or_source(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
        context: &str,
    ) -> Option<(String, Option<WorkspaceSourceRef>)> {
        let base_field = fields.remove("base");
        let source_field = fields.remove("source");
        match (base_field, source_field) {
            (Some(base), Some(source)) => {
                self.errors.push(Diagnostic::error(
                    source.name.span,
                    format!("{context} cannot set both `base` and `source`"),
                ));
                match base.value {
                    CstValue::String(s, _) => Some((s, None)),
                    other @ (CstValue::Integer(..)
                    | CstValue::Duration(..)
                    | CstValue::Bool(..)
                    | CstValue::Null(_)
                    | CstValue::Ident(..)
                    | CstValue::List(..)
                    | CstValue::Block(_)
                    | CstValue::Call { .. }) => {
                        self.errors
                            .push(Diagnostic::error(other.span(), "`base` must be a string"));
                        None
                    }
                }
            }
            (Some(base), None) => match base.value {
                CstValue::String(s, _) => Some((s, None)),
                other @ (CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Bool(..)
                | CstValue::Null(_)
                | CstValue::Ident(..)
                | CstValue::List(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
                    self.errors
                        .push(Diagnostic::error(other.span(), "`base` must be a string"));
                    None
                }
            },
            (None, Some(source)) => match source.value {
                CstValue::String(s, _) => {
                    let source = WorkspaceSourceRef::Path(s.clone());
                    Some((s, Some(source)))
                }
                CstValue::Ident(name, _) => {
                    Some((String::new(), Some(WorkspaceSourceRef::Named(name))))
                }
                other @ (CstValue::Integer(..)
                | CstValue::Duration(..)
                | CstValue::Bool(..)
                | CstValue::Null(_)
                | CstValue::List(..)
                | CstValue::Block(_)
                | CstValue::Call { .. }) => {
                    self.errors.push(Diagnostic::error(
                        other.span(),
                        "`source` must be a source name or path string",
                    ));
                    None
                }
            },
            (None, None) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        format!("{context} requires `base` or `source`"),
                    )
                    .with_hint("add `base = \"...\"`, `source = \"...\"`, or `source = <name>`"),
                );
                None
            }
        }
    }

    /// Parse the required nested `apply_back { mode = ...; excludes = [...];
    /// includes = [...] }` block.
    ///
    /// `mode` is required. `excludes` / `includes` default to `[]`. When
    /// `mode = discard`, non-empty filter lists are a hard error — the lists
    /// would have nowhere to apply.
    pub(super) fn take_required_apply_back_block(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
        context: &str,
    ) -> Option<ApplyBackDef> {
        let Some(field) = fields.remove("apply_back") else {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    format!("{context} requires `apply_back {{ ... }}`"),
                )
                .with_hint(APPLY_BACK_HINT),
            );
            return None;
        };
        let block = match field.value {
            CstValue::Block(block) => block,
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Ident(..)
            | CstValue::List(..)
            | CstValue::Call { .. }) => {
                self.errors.push(
                    Diagnostic::error(other.span(), "`apply_back` must be a block")
                        .with_hint(APPLY_BACK_HINT),
                );
                return None;
            }
        };
        let block_span = block.span.clone();
        let mut inner = self.collect_fields(Some(block));
        let mode = self.take_required_apply_back_mode(&mut inner, &block_span);
        let (excludes, excludes_span) =
            self.take_optional_string_list_with_span(&mut inner, "excludes");
        let (includes, includes_span) =
            self.take_optional_string_list_with_span(&mut inner, "includes");
        self.reject_unknown_fields(
            &mut inner,
            &["mode", "excludes", "includes"],
            "workspace apply_back",
        );

        let excludes_vec = excludes.unwrap_or_default();
        let includes_vec = includes.unwrap_or_default();

        // discard + non-empty filter is a hard error: the lists would have
        // nowhere to apply.
        if let Some(CloneApplyBackMode::Discard) = mode {
            if !excludes_vec.is_empty() {
                let span = excludes_span.unwrap_or_else(|| block_span.clone());
                self.errors.push(
                    Diagnostic::error(
                        span,
                        "`apply_back.excludes` has no effect when `mode = discard`",
                    )
                    .with_hint(APPLY_BACK_DISCARD_FILTER_HINT),
                );
            }
            if !includes_vec.is_empty() {
                let span = includes_span.unwrap_or_else(|| block_span.clone());
                self.errors.push(
                    Diagnostic::error(
                        span,
                        "`apply_back.includes` has no effect when `mode = discard`",
                    )
                    .with_hint(APPLY_BACK_DISCARD_FILTER_HINT),
                );
            }
        }

        Some(ApplyBackDef {
            mode: mode?,
            excludes: excludes_vec,
            includes: includes_vec,
        })
    }

    fn take_required_apply_back_mode(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        block_span: &Span,
    ) -> Option<CloneApplyBackMode> {
        let Some(field) = fields.remove("mode") else {
            self.errors.push(
                Diagnostic::error(block_span.clone(), "`apply_back` block requires `mode`")
                    .with_hint(APPLY_BACK_MODE_HINT),
            );
            return None;
        };
        match field.value {
            CstValue::Ident(ident, span) => self.parse_clone_apply_back(&ident, span),
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::List(..)
            | CstValue::Block(_)
            | CstValue::Call { .. }) => {
                self.errors.push(
                    Diagnostic::error(other.span(), "`mode` must be an identifier")
                        .with_hint(APPLY_BACK_MODE_HINT),
                );
                None
            }
        }
    }

    /// Like [`Analyzer::take_optional_string_list`] but also returns the
    /// span of the field's value so callers can pin diagnostics on the
    /// exact list literal.
    fn take_optional_string_list_with_span(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> (Option<Vec<String>>, Option<Span>) {
        let Some(field) = fields.remove(name) else {
            return (None, None);
        };
        let span = field.value.span();
        match field.value {
            CstValue::List(items, _) => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        CstValue::String(s, _) => out.push(s),
                        other @ (CstValue::Integer(..)
                        | CstValue::Duration(..)
                        | CstValue::Bool(..)
                        | CstValue::Null(_)
                        | CstValue::Ident(..)
                        | CstValue::List(..)
                        | CstValue::Block(_)
                        | CstValue::Call { .. }) => self.errors.push(Diagnostic::error(
                            other.span(),
                            format!("`{name}` list elements must be strings"),
                        )),
                    }
                }
                (Some(out), Some(span))
            }
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Ident(..)
            | CstValue::Block(_)
            | CstValue::Call { .. }) => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    format!("`{name}` must be a list of strings"),
                ));
                (None, None)
            }
        }
    }

    pub(super) fn take_required_sandbox_policy(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SandboxPolicyDef> {
        let Some(field) = fields.remove("policy") else {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "workspace sandbox requires `policy { ... }`",
                )
                .with_hint(POLICY_HINT),
            );
            return None;
        };
        let block = match field.value {
            CstValue::Block(block) => block,
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Ident(..)
            | CstValue::List(..)
            | CstValue::Call { .. }) => {
                self.errors
                    .push(Diagnostic::error(other.span(), "`policy` must be a block"));
                return None;
            }
        };
        let policy_span = block.span.clone();
        let mut policy_fields = self.collect_fields(Some(block));
        let network = self.take_required_sandbox_network(&mut policy_fields, &policy_span);
        let allow_read_outside = self
            .take_optional_string_list(&mut policy_fields, "allow_read_outside")
            .unwrap_or_default();
        let allow_write_outside = self
            .take_optional_string_list(&mut policy_fields, "allow_write_outside")
            .unwrap_or_default();
        let extra_deny_paths = self
            .take_optional_string_list(&mut policy_fields, "extra_deny_paths")
            .unwrap_or_default();
        let allow_exec = self
            .take_optional_string_list(&mut policy_fields, "allow_exec")
            .unwrap_or_default();
        self.reject_unknown_fields(
            &mut policy_fields,
            &[
                "network",
                "allow_read_outside",
                "allow_write_outside",
                "extra_deny_paths",
                "allow_exec",
            ],
            "workspace sandbox policy",
        );
        Some(SandboxPolicyDef {
            network: network?,
            allow_read_outside,
            allow_write_outside,
            extra_deny_paths,
            allow_exec,
        })
    }

    pub(super) fn take_required_sandbox_network(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        policy_span: &Span,
    ) -> Option<SandboxNetworkDef> {
        let Some(field) = fields.remove("network") else {
            self.errors.push(
                Diagnostic::error(policy_span.clone(), "`policy` block requires `network`")
                    .with_hint(NETWORK_HINT),
            );
            return None;
        };
        match field.value {
            CstValue::Ident(name, span) => match name.as_str() {
                "off" => Some(SandboxNetworkDef::Off),
                "all" => Some(SandboxNetworkDef::All),
                other => {
                    self.errors.push(
                        Diagnostic::error(span, format!("unknown network mode `{other}`"))
                            .with_hint("valid modes: off, all, or a list of host strings"),
                    );
                    None
                }
            },
            CstValue::List(items, _) => {
                let mut hosts = Vec::new();
                for item in items {
                    match item {
                        CstValue::String(s, _) => hosts.push(s),
                        other @ (CstValue::Integer(..)
                        | CstValue::Duration(..)
                        | CstValue::Bool(..)
                        | CstValue::Null(_)
                        | CstValue::Ident(..)
                        | CstValue::List(..)
                        | CstValue::Block(_)
                        | CstValue::Call { .. }) => self.errors.push(Diagnostic::error(
                            other.span(),
                            "`network` host entries must be strings",
                        )),
                    }
                }
                Some(SandboxNetworkDef::Hosts(hosts))
            }
            other @ (CstValue::String(..)
            | CstValue::Integer(..)
            | CstValue::Duration(..)
            | CstValue::Bool(..)
            | CstValue::Null(_)
            | CstValue::Block(_)
            | CstValue::Call { .. }) => {
                self.errors.push(Diagnostic::error(
                    other.span(),
                    "`network` must be `off`, `all`, or a list of host strings",
                ));
                None
            }
        }
    }
}
