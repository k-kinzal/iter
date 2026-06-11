//! `queue pubsub { ... }` lowerer (GCP Pub/Sub).
//!
//! Mirrors the [`super::sqs`] shape: take required identity fields,
//! optional sub-blocks (credentials, publisher, subscriber, keepalive,
//! `initial_seek`, dlq), and surface mutual-exclusion violations as
//! diagnostics.

use std::collections::BTreeMap;

use super::super::Analyzer;
use crate::ast::{
    PubSubConfig, PubSubCredentialKind, PubSubCredentials, PubSubInitialSeek, PubSubKeepalive,
    PubSubPublisher, PubSubSubscriber, Span,
};
use crate::diagnostic::Diagnostic;
use crate::parser::CstField;

const PUBSUB_FIELDS: &[&str] = &[
    "project",
    "topic",
    "subscription",
    "endpoint",
    "user_agent",
    "connect_timeout",
    "request_timeout",
    "keepalive",
    "quota_project",
    "scopes",
    "credentials",
    "publisher",
    "subscriber",
    "initial_seek",
    "dlq",
];

const PUBSUB_KEEPALIVE_FIELDS: &[&str] = &["time", "timeout", "permit_without_stream"];

const PUBSUB_PUBLISHER_FIELDS: &[&str] = &[
    "delay_threshold",
    "count_threshold",
    "byte_threshold",
    "max_outstanding_messages",
    "max_outstanding_bytes",
    "limit_exceeded_behavior",
    "workers",
    "request_timeout",
    "retry",
    "enable_compression",
    "compression_bytes_threshold",
    "attributes",
    "ordering_key_strategy",
];

const PUBSUB_SUBSCRIBER_FIELDS: &[&str] = &[
    "pull_mode",
    "stream_ack_deadline_seconds",
    "max_outstanding_messages",
    "max_outstanding_bytes",
    "min_duration_per_lease_extension",
    "max_duration_per_lease_extension",
    "ping_interval",
    "max_messages",
    "return_immediately",
    "retry",
];

const PUBSUB_INITIAL_SEEK_FIELDS: &[&str] = &["kind", "timestamp", "snapshot_name"];

const PUBSUB_CRED_DEFAULT: &[&str] = &["kind"];
const PUBSUB_CRED_SA_FILE: &[&str] = &["kind", "path"];
const PUBSUB_CRED_SA_INLINE: &[&str] = &["kind", "json"];
const PUBSUB_CRED_WORKLOAD: &[&str] = &["kind", "audience", "token_file", "impersonation_target"];
const PUBSUB_CRED_IMPERSONATE: &[&str] = &["kind", "target_principal", "delegates", "scopes"];
const PUBSUB_CRED_TOKEN: &[&str] = &["kind", "token", "expiry"];

impl Analyzer {
    pub(super) fn lower_pubsub(
        &mut self,
        body: BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> PubSubConfig {
        let mut fields = body;
        let project = self
            .take_required_string(&mut fields, "project", kind_span, "queue pubsub")
            .unwrap_or_default();
        let topic = self
            .take_required_string(&mut fields, "topic", kind_span, "queue pubsub")
            .unwrap_or_default();
        let subscription = self
            .take_required_string(&mut fields, "subscription", kind_span, "queue pubsub")
            .unwrap_or_default();
        let endpoint = self.take_optional_string(&mut fields, "endpoint");
        let user_agent = self.take_optional_string(&mut fields, "user_agent");
        let connect_timeout_secs = self.take_optional_duration(&mut fields, "connect_timeout");
        let request_timeout_secs = self.take_optional_duration(&mut fields, "request_timeout");
        let keepalive = self.lower_pubsub_keepalive(&mut fields);
        let quota_project = self.take_optional_string(&mut fields, "quota_project");
        let scopes = self.take_optional_string_list(&mut fields, "scopes");
        let credentials = self.lower_pubsub_credentials(&mut fields, kind_span);
        let publisher = self.lower_pubsub_publisher(&mut fields);
        let subscriber = self.lower_pubsub_subscriber(&mut fields);
        let initial_seek = self.lower_pubsub_initial_seek(&mut fields, kind_span);
        let dlq = self.lower_dlq_policy(&mut fields, "dlq", kind_span);

        self.reject_unknown_fields(&mut fields, PUBSUB_FIELDS, "queue pubsub");

        PubSubConfig {
            project,
            topic,
            subscription,
            endpoint,
            user_agent,
            connect_timeout_secs,
            request_timeout_secs,
            keepalive,
            quota_project,
            scopes,
            credentials,
            publisher,
            subscriber,
            initial_seek,
            dlq,
        }
    }

    fn lower_pubsub_keepalive(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<PubSubKeepalive> {
        let mut block = self.take_optional_block(fields, "keepalive")?;
        let time_secs = self.take_optional_duration(&mut block, "time");
        let timeout_secs = self.take_optional_duration(&mut block, "timeout");
        let permit_without_stream = self.take_optional_bool(&mut block, "permit_without_stream");
        self.reject_unknown_fields(&mut block, PUBSUB_KEEPALIVE_FIELDS, "pubsub keepalive");
        Some(PubSubKeepalive {
            time_secs,
            timeout_secs,
            permit_without_stream,
        })
    }

    fn lower_pubsub_credentials(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentials> {
        let mut block = self.take_optional_block(fields, "credentials")?;
        let Some(kind) = self.take_optional_string(&mut block, "kind") else {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "credentials block requires `kind = \"...\"`",
                )
                .with_hint(
                    "valid kinds: adc, service_account_file, service_account_inline, workload_identity, impersonate, access_token",
                ),
            );
            return None;
        };
        let lowered = match kind.as_str() {
            "adc" => {
                self.reject_unknown_fields(&mut block, PUBSUB_CRED_DEFAULT, "credentials adc");
                Some(PubSubCredentialKind::Adc)
            }
            "service_account_file" => self.lower_pubsub_cred_sa_file(&mut block, kind_span),
            "service_account_inline" => self.lower_pubsub_cred_sa_inline(&mut block, kind_span),
            "workload_identity" => self.lower_pubsub_cred_workload(&mut block, kind_span),
            "impersonate" => self.lower_pubsub_cred_impersonate(&mut block, kind_span),
            "access_token" => self.lower_pubsub_cred_access_token(&mut block, kind_span),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        format!("unknown credentials kind `{other}`"),
                    )
                    .with_hint(
                        "valid kinds: adc, service_account_file, service_account_inline, workload_identity, impersonate, access_token",
                    ),
                );
                None
            }
        };
        lowered.map(|kind| PubSubCredentials { kind })
    }

    fn lower_pubsub_cred_sa_file(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentialKind> {
        let path =
            self.take_required_string(block, "path", kind_span, "credentials service_account_file");
        self.reject_unknown_fields(
            block,
            PUBSUB_CRED_SA_FILE,
            "credentials service_account_file",
        );
        Some(PubSubCredentialKind::ServiceAccountFile { path: path? })
    }

    fn lower_pubsub_cred_sa_inline(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentialKind> {
        let json = self.take_required_secret(
            block,
            "json",
            kind_span,
            "credentials service_account_inline",
        );
        self.reject_unknown_fields(
            block,
            PUBSUB_CRED_SA_INLINE,
            "credentials service_account_inline",
        );
        Some(PubSubCredentialKind::ServiceAccountInline { json: json? })
    }

    fn lower_pubsub_cred_workload(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentialKind> {
        let audience = self.take_required_string(
            block,
            "audience",
            kind_span,
            "credentials workload_identity",
        );
        let token_file = self.take_required_string(
            block,
            "token_file",
            kind_span,
            "credentials workload_identity",
        );
        let impersonation_target = self.take_optional_string(block, "impersonation_target");
        self.reject_unknown_fields(block, PUBSUB_CRED_WORKLOAD, "credentials workload_identity");
        Some(PubSubCredentialKind::WorkloadIdentity {
            audience: audience?,
            token_file: token_file?,
            impersonation_target,
        })
    }

    fn lower_pubsub_cred_impersonate(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentialKind> {
        let target_principal = self.take_required_string(
            block,
            "target_principal",
            kind_span,
            "credentials impersonate",
        );
        let delegates = self.take_optional_string_list(block, "delegates");
        let scopes = self.take_optional_string_list(block, "scopes");
        self.reject_unknown_fields(block, PUBSUB_CRED_IMPERSONATE, "credentials impersonate");
        Some(PubSubCredentialKind::Impersonate {
            target_principal: target_principal?,
            delegates,
            scopes,
        })
    }

    fn lower_pubsub_cred_access_token(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubCredentialKind> {
        let token =
            self.take_required_secret(block, "token", kind_span, "credentials access_token");
        let expiry = self.take_optional_string(block, "expiry");
        self.reject_unknown_fields(block, PUBSUB_CRED_TOKEN, "credentials access_token");
        Some(PubSubCredentialKind::AccessToken {
            token: token?,
            expiry,
        })
    }

    fn lower_pubsub_publisher(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<PubSubPublisher> {
        let mut block = self.take_optional_block(fields, "publisher")?;
        let delay_threshold_ms = self.take_optional_int(&mut block, "delay_threshold");
        let count_threshold = self.take_optional_int(&mut block, "count_threshold");
        let byte_threshold = self.take_optional_int(&mut block, "byte_threshold");
        let max_outstanding_messages =
            self.take_optional_int(&mut block, "max_outstanding_messages");
        let max_outstanding_bytes = self.take_optional_int(&mut block, "max_outstanding_bytes");
        let limit_exceeded_behavior =
            self.take_optional_string(&mut block, "limit_exceeded_behavior");
        let workers = self.take_optional_int(&mut block, "workers");
        let request_timeout_secs = self.take_optional_duration(&mut block, "request_timeout");
        let retry = self.lower_retry_policy(&mut block, "retry");
        let enable_compression = self.take_optional_bool(&mut block, "enable_compression");
        let compression_bytes_threshold =
            self.take_optional_int(&mut block, "compression_bytes_threshold");
        let attributes = self.take_optional_string_string_block(&mut block, "attributes");
        let ordering_key_strategy =
            self.take_optional_templated_string(&mut block, "ordering_key_strategy");
        self.reject_unknown_fields(&mut block, PUBSUB_PUBLISHER_FIELDS, "pubsub publisher");
        Some(PubSubPublisher {
            delay_threshold_ms,
            count_threshold,
            byte_threshold,
            max_outstanding_messages,
            max_outstanding_bytes,
            limit_exceeded_behavior,
            workers,
            request_timeout_secs,
            retry,
            enable_compression,
            compression_bytes_threshold,
            attributes,
            ordering_key_strategy,
        })
    }

    fn lower_pubsub_subscriber(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<PubSubSubscriber> {
        let mut block = self.take_optional_block(fields, "subscriber")?;
        let pull_mode = self.take_optional_string(&mut block, "pull_mode");
        let stream_ack_deadline_seconds =
            self.take_optional_int(&mut block, "stream_ack_deadline_seconds");
        let max_outstanding_messages =
            self.take_optional_int(&mut block, "max_outstanding_messages");
        let max_outstanding_bytes = self.take_optional_int(&mut block, "max_outstanding_bytes");
        let min_duration_per_lease_extension_secs =
            self.take_optional_duration(&mut block, "min_duration_per_lease_extension");
        let max_duration_per_lease_extension_secs =
            self.take_optional_duration(&mut block, "max_duration_per_lease_extension");
        let ping_interval_secs = self.take_optional_duration(&mut block, "ping_interval");
        let max_messages = self.take_optional_int(&mut block, "max_messages");
        let return_immediately = self.take_optional_bool(&mut block, "return_immediately");
        let retry = self.lower_retry_policy(&mut block, "retry");
        self.reject_unknown_fields(&mut block, PUBSUB_SUBSCRIBER_FIELDS, "pubsub subscriber");
        Some(PubSubSubscriber {
            pull_mode,
            stream_ack_deadline_seconds,
            max_outstanding_messages,
            max_outstanding_bytes,
            min_duration_per_lease_extension_secs,
            max_duration_per_lease_extension_secs,
            ping_interval_secs,
            max_messages,
            return_immediately,
            retry,
        })
    }

    fn lower_pubsub_initial_seek(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<PubSubInitialSeek> {
        let mut block = self.take_optional_block(fields, "initial_seek")?;
        let kind = self.take_required_string(&mut block, "kind", kind_span, "pubsub initial_seek");
        let timestamp = self.take_optional_string(&mut block, "timestamp");
        let snapshot_name = self.take_optional_string(&mut block, "snapshot_name");
        self.reject_unknown_fields(
            &mut block,
            PUBSUB_INITIAL_SEEK_FIELDS,
            "pubsub initial_seek",
        );
        let kind = kind?;
        match kind.as_str() {
            "timestamp" => {
                if timestamp.is_none() {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "initial_seek kind `timestamp` requires `timestamp`",
                    ));
                }
            }
            "snapshot" => {
                if snapshot_name.is_none() {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "initial_seek kind `snapshot` requires `snapshot_name`",
                    ));
                }
            }
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        format!("unknown initial_seek kind `{other}`"),
                    )
                    .with_hint("valid kinds: timestamp, snapshot"),
                );
                return None;
            }
        }
        Some(PubSubInitialSeek {
            kind,
            timestamp,
            snapshot_name,
        })
    }
}
