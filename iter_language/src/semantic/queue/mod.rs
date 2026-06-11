//! `queue { ... }` lowerer and shared retry/DLQ helpers.
//!
//! The per-backend lowerers live in sibling modules; this module holds
//! the top-level dispatch plus the retry/DLQ helpers that every backend
//! with an SDK retry/dead-letter surface shares.

pub(super) mod sqs;

use std::collections::BTreeMap;

use super::Analyzer;
use super::TemplatePosition;
use crate::ast::{DlqPolicyDef, DlqTargetDef, QueueDef, RetryPolicyDef, Span};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstField, CstIdent};

const RETRY_FIELDS: &[&str] = &[
    "mode",
    "max_attempts",
    "initial_backoff",
    "max_backoff",
    "try_timeout",
    "retryable_codes",
];

const DLQ_FIELDS: &[&str] = &[
    "kind",
    "max_receive_count",
    "reason_template",
    "include_headers",
    "target",
];

impl Analyzer {
    pub(super) fn lower_queue(
        &mut self,
        kind: Option<CstIdent>,
        body: Option<CstBlock>,
        keyword_span: &Span,
    ) -> Option<QueueDef> {
        let kind = self.require_kind(
            kind,
            keyword_span,
            "queue",
            &["memory", "file", "redis", "shell", "sqs"],
        )?;
        let mut fields = self.collect_fields(body);
        let decl = match kind.name.as_str() {
            "memory" => {
                self.reject_unknown_fields(&mut fields, &[], "queue memory");
                QueueDef::Memory
            }
            "file" => {
                let path = self.take_required_string(&mut fields, "path", &kind.span, "queue file");
                self.reject_unknown_fields(&mut fields, &["path"], "queue file");
                QueueDef::File { path: path? }
            }
            "redis" => {
                let url = self.take_required_string(&mut fields, "url", &kind.span, "queue redis");
                let key = self.take_required_string(&mut fields, "key", &kind.span, "queue redis");
                self.reject_unknown_fields(&mut fields, &["url", "key"], "queue redis");
                QueueDef::Redis {
                    url: url?,
                    key: key?,
                }
            }
            "shell" => {
                let enqueue =
                    self.take_required_string(&mut fields, "enqueue", &kind.span, "queue shell");
                let dequeue =
                    self.take_required_string(&mut fields, "dequeue", &kind.span, "queue shell");
                let close = self.take_optional_string(&mut fields, "close");
                let interpreter = self.take_optional_string(&mut fields, "interpreter");
                let enqueue_timeout_secs =
                    self.take_optional_duration(&mut fields, "enqueue_timeout");
                self.reject_unknown_fields(
                    &mut fields,
                    &[
                        "enqueue",
                        "dequeue",
                        "close",
                        "interpreter",
                        "enqueue_timeout",
                    ],
                    "queue shell",
                );
                QueueDef::Shell {
                    enqueue: enqueue?,
                    dequeue: dequeue?,
                    close,
                    interpreter,
                    enqueue_timeout_secs,
                }
            }
            "sqs" => {
                let cfg = self.lower_sqs(std::mem::take(&mut fields), &kind.span);
                QueueDef::Sqs(Box::new(cfg))
            }
            other => {
                self.errors.push(
                    Diagnostic::error(kind.span, format!("unknown queue kind `{other}`"))
                        .with_hint("valid kinds: memory, file, redis, shell, sqs"),
                );
                return None;
            }
        };
        Some(decl)
    }

    pub(in crate::semantic) fn lower_retry_policy(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
    ) -> Option<RetryPolicyDef> {
        let mut block = self.take_optional_block(fields, name)?;
        let mode = self.take_optional_string(&mut block, "mode");
        let max_attempts = self.take_optional_int(&mut block, "max_attempts");
        let initial_backoff_secs = self.take_optional_duration(&mut block, "initial_backoff");
        let max_backoff_secs = self.take_optional_duration(&mut block, "max_backoff");
        let try_timeout_secs = self.take_optional_duration(&mut block, "try_timeout");
        let retryable_codes = self.take_optional_string_list(&mut block, "retryable_codes");
        self.reject_unknown_fields(&mut block, RETRY_FIELDS, "retry");
        Some(RetryPolicyDef {
            mode,
            max_attempts,
            initial_backoff_secs,
            max_backoff_secs,
            try_timeout_secs,
            retryable_codes,
        })
    }

    pub(in crate::semantic) fn lower_dlq_policy(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
    ) -> Option<DlqPolicyDef> {
        let mut block = self.take_optional_block(fields, name)?;
        let kind = self.take_optional_string(&mut block, "kind");
        let max_receive_count = self.take_optional_int(&mut block, "max_receive_count");
        let reason_template = self.take_optional_template_text(
            &mut block,
            "reason_template",
            TemplatePosition::DeadLetterReason,
        );
        let include_headers = self.take_optional_bool(&mut block, "include_headers");
        let target = self.lower_dlq_target(&mut block, "target", kind_span);
        self.reject_unknown_fields(&mut block, DLQ_FIELDS, "dlq");

        // iter_republish requires `target` and `max_receive_count` — surface
        // a single combined diagnostic rather than failing silently.
        if let Some(k) = kind.as_deref() {
            if k == "iter_republish" {
                if target.is_none() {
                    self.errors.push(
                        Diagnostic::error(
                            kind_span.clone(),
                            "dlq kind `iter_republish` requires a `target { ... }` block",
                        )
                        .with_hint("add `target { kind = \"sqs\" queue_url = \"...\" }`"),
                    );
                }
                if max_receive_count.is_none() {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "dlq kind `iter_republish` requires `max_receive_count`",
                    ));
                }
            }
        }

        Some(DlqPolicyDef {
            kind,
            max_receive_count,
            reason_template,
            include_headers,
            target,
        })
    }

    fn lower_dlq_target(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        name: &str,
        kind_span: &Span,
    ) -> Option<DlqTargetDef> {
        let mut block = self.take_optional_block(fields, name)?;
        let Some(kind) = self.take_optional_string(&mut block, "kind") else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), "dlq target requires `kind = \"...\"`")
                    .with_hint("valid kinds: sqs, s3, file"),
            );
            return None;
        };
        match kind.as_str() {
            "sqs" => self.lower_dlq_sqs(&mut block, kind_span),
            "s3" => self.lower_dlq_s3(&mut block, kind_span),
            "file" => self.lower_dlq_file(&mut block, kind_span),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        format!("unknown dlq target kind `{other}`"),
                    )
                    .with_hint("valid kinds: sqs, s3, file"),
                );
                None
            }
        }
    }

    fn lower_dlq_sqs(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<DlqTargetDef> {
        let queue_url = self.take_required_string(block, "queue_url", kind_span, "dlq target sqs");
        let region = self.take_optional_string(block, "region");
        self.reject_unknown_fields(block, &["kind", "queue_url", "region"], "dlq target sqs");
        Some(DlqTargetDef::Sqs {
            queue_url: queue_url?,
            region,
        })
    }

    fn lower_dlq_s3(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<DlqTargetDef> {
        let bucket = self.take_required_string(block, "bucket", kind_span, "dlq target s3");
        let prefix = self.take_optional_string(block, "prefix");
        let region = self.take_optional_string(block, "region");
        self.reject_unknown_fields(
            block,
            &["kind", "bucket", "prefix", "region"],
            "dlq target s3",
        );
        Some(DlqTargetDef::S3 {
            bucket: bucket?,
            prefix,
            region,
        })
    }

    fn lower_dlq_file(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<DlqTargetDef> {
        let path = self.take_required_string(block, "path", kind_span, "dlq target file");
        self.reject_unknown_fields(block, &["kind", "path"], "dlq target file");
        Some(DlqTargetDef::File { path: path? })
    }
}
