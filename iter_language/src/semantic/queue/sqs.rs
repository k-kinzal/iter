//! `queue sqs { ... }` lowerer.
//!
//! Lives in its own file because the SQS surface is large enough that
//! folding it into [`super`] makes the dispatch unreadable. The
//! lowerer handles every nested sub-block (`credentials { ... }`,
//! `http_client { ... }`, `producer { ... }`, `consumer { ... }`,
//! `retry { ... }`, `dlq { ... }`) and enforces the structural mutual-
//! exclusion rules (`queue_url` xor `queue_name`+`account_id`,
//! `credentials.kind` discriminator).

use std::collections::BTreeMap;

use super::super::Analyzer;
use crate::ast::{
    Span, SqsConfig, SqsConsumer, SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity,
    SqsProducer,
};
use crate::diagnostic::Diagnostic;
use crate::parser::CstField;

const SQS_FIELDS: &[&str] = &[
    "queue_url",
    "queue_name",
    "account_id",
    "region",
    "endpoint_url",
    "fifo",
    "use_fips",
    "use_dual_stack",
    "sts_regional_endpoints",
    "app_name",
    "credentials",
    "http_client",
    "producer",
    "consumer",
    "retry",
    "dlq",
];

const CRED_FIELDS_DEFAULT: &[&str] = &["kind"];
const CRED_FIELDS_STATIC: &[&str] = &[
    "kind",
    "access_key_id",
    "secret_access_key",
    "session_token",
];
const CRED_FIELDS_ASSUME_ROLE: &[&str] = &[
    "kind",
    "role_arn",
    "session_name",
    "external_id",
    "duration_seconds",
    "source_profile",
];
const CRED_FIELDS_PROFILE: &[&str] = &["kind", "name"];
const CRED_FIELDS_WEB_IDENTITY: &[&str] = &["kind", "role_arn", "token_file", "session_name"];
const CRED_FIELDS_IMDS: &[&str] = &["kind"];
const CRED_FIELDS_PROCESS: &[&str] = &["kind", "command"];

const HTTP_CLIENT_FIELDS: &[&str] = &[
    "connect_timeout",
    "read_timeout",
    "operation_timeout",
    "operation_attempt_timeout",
    "tcp_keepalive",
    "max_idle_connections_per_host",
    "connection_pool_idle_timeout",
    "proxy_url",
    "no_proxy",
];

const PRODUCER_FIELDS: &[&str] = &[
    "delay_seconds",
    "message_attributes",
    "trace_header",
    "message_group_id",
    "message_deduplication_id",
    "batch_size",
    "batch_max_bytes",
    "batch_linger",
];

const CONSUMER_FIELDS: &[&str] = &[
    "visibility_timeout",
    "wait_time_seconds",
    "max_number_of_messages",
    "message_attribute_names",
    "message_system_attribute_names",
    "concurrent_receivers",
];

impl Analyzer {
    pub(in crate::semantic) fn lower_sqs(
        &mut self,
        body: BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> SqsConfig {
        let mut fields = body;
        let identity = self.lower_sqs_identity(&mut fields, kind_span);
        let region = self.take_optional_string(&mut fields, "region");
        let endpoint_url = self.take_optional_string(&mut fields, "endpoint_url");
        let fifo = self.take_optional_bool(&mut fields, "fifo");
        let use_fips = self.take_optional_bool(&mut fields, "use_fips");
        let use_dual_stack = self.take_optional_bool(&mut fields, "use_dual_stack");
        let sts_regional_endpoints =
            self.take_optional_string(&mut fields, "sts_regional_endpoints");
        let app_name = self.take_optional_string(&mut fields, "app_name");

        let credentials = self.lower_sqs_credentials(&mut fields, kind_span);
        let http_client = self.lower_sqs_http_client(&mut fields);
        let producer = self.lower_sqs_producer(&mut fields);
        let consumer = self.lower_sqs_consumer(&mut fields);
        let retry = self.lower_retry_policy(&mut fields, "retry");
        let dlq = self.lower_dlq_policy(&mut fields, "dlq", kind_span);

        self.reject_unknown_fields(&mut fields, SQS_FIELDS, "queue sqs");

        self.validate_sqs_fifo_only_fields(kind_span, &identity, fifo, producer.as_ref());

        SqsConfig {
            identity,
            region,
            endpoint_url,
            fifo,
            use_fips,
            use_dual_stack,
            sts_regional_endpoints,
            app_name,
            credentials,
            http_client,
            producer,
            consumer,
            retry,
            dlq,
        }
    }

    /// Reject `message_group_id` / `message_deduplication_id` on non-FIFO
    /// queues. SQS rejects them at runtime, so we surface the conflict at
    /// lowering time with a more actionable diagnostic.
    fn validate_sqs_fifo_only_fields(
        &mut self,
        kind_span: &Span,
        identity: &SqsIdentity,
        fifo_field: Option<bool>,
        producer: Option<&SqsProducer>,
    ) {
        let Some(producer) = producer else { return };
        if producer.message_group_id.is_none() && producer.message_deduplication_id.is_none() {
            return;
        }
        if Self::sqs_identity_is_fifo(identity, fifo_field) {
            return;
        }
        let bad_fields: Vec<&'static str> = [
            ("message_group_id", producer.message_group_id.is_some()),
            (
                "message_deduplication_id",
                producer.message_deduplication_id.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(n, set)| set.then_some(n))
        .collect();
        self.errors.push(
            Diagnostic::error(
                kind_span.clone(),
                format!(
                    "queue sqs: producer field(s) {} require a FIFO queue",
                    bad_fields
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_hint(
                "use a `.fifo` queue URL/name or set `fifo = true`; otherwise drop these fields",
            ),
        );
    }

    fn sqs_identity_is_fifo(identity: &SqsIdentity, fifo_field: Option<bool>) -> bool {
        if let Some(b) = fifo_field {
            return b;
        }
        match identity {
            SqsIdentity::Url(u) => u.as_bytes().ends_with(b".fifo"),
            SqsIdentity::NameWithAccount { name, .. } => name.as_bytes().ends_with(b".fifo"),
            SqsIdentity::Unset => false,
        }
    }

    fn lower_sqs_identity(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> SqsIdentity {
        let url = self.take_optional_string(fields, "queue_url");
        let name = self.take_optional_string(fields, "queue_name");
        let account = self.take_optional_string(fields, "account_id");
        match (url, name, account) {
            (Some(url), None, None) => SqsIdentity::Url(url),
            (None, Some(name), Some(account_id)) => {
                SqsIdentity::NameWithAccount { name, account_id }
            }
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        "queue sqs: `queue_url` and `queue_name`/`account_id` are mutually exclusive",
                    )
                    .with_hint("set either `queue_url` OR both `queue_name` and `account_id`"),
                );
                SqsIdentity::Unset
            }
            (None, Some(_), None) | (None, None, Some(_)) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        "queue sqs: `queue_name` requires `account_id` (and vice versa)",
                    )
                    .with_hint("supply both, or use `queue_url` instead"),
                );
                SqsIdentity::Unset
            }
            (None, None, None) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        "queue sqs requires `queue_url` or `queue_name` + `account_id`",
                    )
                    .with_hint(
                        "add `queue_url = \"https://sqs.<region>.amazonaws.com/<account>/<name>\"`",
                    ),
                );
                SqsIdentity::Unset
            }
        }
    }

    pub(in crate::semantic) fn lower_sqs_credentials(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentials> {
        let mut block = self.take_optional_block(fields, "credentials")?;
        let Some(kind) = self.take_optional_string(&mut block, "kind") else {
            self.errors.push(
                Diagnostic::error(
                    kind_span.clone(),
                    "credentials block requires `kind = \"...\"`",
                )
                .with_hint(
                    "valid kinds: default, static, assume_role, profile, web_identity_token_file, imds, process",
                ),
            );
            return None;
        };
        let lowered = match kind.as_str() {
            "default" => {
                self.reject_unknown_fields(&mut block, CRED_FIELDS_DEFAULT, "credentials default");
                Some(SqsCredentialKind::Default)
            }
            "static" => self.lower_sqs_credentials_static(&mut block, kind_span),
            "assume_role" => self.lower_sqs_credentials_assume_role(&mut block, kind_span),
            "profile" => self.lower_sqs_credentials_profile(&mut block, kind_span),
            "web_identity_token_file" => {
                self.lower_sqs_credentials_web_identity(&mut block, kind_span)
            }
            "imds" => {
                self.reject_unknown_fields(&mut block, CRED_FIELDS_IMDS, "credentials imds");
                Some(SqsCredentialKind::Imds)
            }
            "process" => self.lower_sqs_credentials_process(&mut block, kind_span),
            other => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        format!("unknown credentials kind `{other}`"),
                    )
                    .with_hint(
                        "valid kinds: default, static, assume_role, profile, web_identity_token_file, imds, process",
                    ),
                );
                None
            }
        };
        lowered.map(|kind| SqsCredentials { kind })
    }

    fn lower_sqs_credentials_static(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentialKind> {
        let access_key_id =
            self.take_required_secret(block, "access_key_id", kind_span, "credentials static");
        let secret_access_key =
            self.take_required_secret(block, "secret_access_key", kind_span, "credentials static");
        let session_token = self.take_optional_secret(block, "session_token");
        self.reject_unknown_fields(block, CRED_FIELDS_STATIC, "credentials static");
        Some(SqsCredentialKind::Static {
            access_key_id: access_key_id?,
            secret_access_key: secret_access_key?,
            session_token,
        })
    }

    fn lower_sqs_credentials_assume_role(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentialKind> {
        let role_arn =
            self.take_required_string(block, "role_arn", kind_span, "credentials assume_role");
        let session_name = self.take_optional_string(block, "session_name");
        let external_id = self.take_optional_secret(block, "external_id");
        let duration_seconds = self.take_optional_duration(block, "duration_seconds");
        let source_profile = self.take_optional_string(block, "source_profile");
        self.reject_unknown_fields(block, CRED_FIELDS_ASSUME_ROLE, "credentials assume_role");
        Some(SqsCredentialKind::AssumeRole {
            role_arn: role_arn?,
            session_name,
            external_id,
            duration_seconds,
            source_profile,
        })
    }

    fn lower_sqs_credentials_profile(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentialKind> {
        let name = self.take_required_string(block, "name", kind_span, "credentials profile");
        self.reject_unknown_fields(block, CRED_FIELDS_PROFILE, "credentials profile");
        Some(SqsCredentialKind::Profile { name: name? })
    }

    fn lower_sqs_credentials_web_identity(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentialKind> {
        let role_arn = self.take_required_string(
            block,
            "role_arn",
            kind_span,
            "credentials web_identity_token_file",
        );
        let token_file = self.take_required_string(
            block,
            "token_file",
            kind_span,
            "credentials web_identity_token_file",
        );
        let session_name = self.take_optional_string(block, "session_name");
        self.reject_unknown_fields(
            block,
            CRED_FIELDS_WEB_IDENTITY,
            "credentials web_identity_token_file",
        );
        Some(SqsCredentialKind::WebIdentityTokenFile {
            role_arn: role_arn?,
            token_file: token_file?,
            session_name,
        })
    }

    fn lower_sqs_credentials_process(
        &mut self,
        block: &mut BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> Option<SqsCredentialKind> {
        let command = self.take_required_string(block, "command", kind_span, "credentials process");
        self.reject_unknown_fields(block, CRED_FIELDS_PROCESS, "credentials process");
        Some(SqsCredentialKind::Process { command: command? })
    }

    pub(in crate::semantic) fn lower_sqs_http_client(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<SqsHttpClient> {
        let mut block = self.take_optional_block(fields, "http_client")?;
        let connect_timeout_secs = self.take_optional_duration(&mut block, "connect_timeout");
        let read_timeout_secs = self.take_optional_duration(&mut block, "read_timeout");
        let operation_timeout_secs = self.take_optional_duration(&mut block, "operation_timeout");
        let operation_attempt_timeout_secs =
            self.take_optional_duration(&mut block, "operation_attempt_timeout");
        let tcp_keepalive_secs = self.take_optional_duration(&mut block, "tcp_keepalive");
        let max_idle_connections_per_host =
            self.take_optional_int(&mut block, "max_idle_connections_per_host");
        let connection_pool_idle_timeout_secs =
            self.take_optional_duration(&mut block, "connection_pool_idle_timeout");
        let proxy_url = self.take_optional_string(&mut block, "proxy_url");
        let no_proxy = self.take_optional_string_list(&mut block, "no_proxy");
        self.reject_unknown_fields(&mut block, HTTP_CLIENT_FIELDS, "http_client");
        Some(SqsHttpClient {
            connect_timeout_secs,
            read_timeout_secs,
            operation_timeout_secs,
            operation_attempt_timeout_secs,
            tcp_keepalive_secs,
            max_idle_connections_per_host,
            connection_pool_idle_timeout_secs,
            proxy_url,
            no_proxy,
        })
    }

    fn lower_sqs_producer(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<SqsProducer> {
        let mut block = self.take_optional_block(fields, "producer")?;
        let delay_seconds = self.take_optional_duration(&mut block, "delay_seconds");
        let message_attributes =
            self.take_optional_string_string_block(&mut block, "message_attributes");
        let trace_header = self.take_optional_bool(&mut block, "trace_header");
        let message_group_id = self.take_optional_templated_string(&mut block, "message_group_id");
        let message_deduplication_id =
            self.take_optional_templated_string(&mut block, "message_deduplication_id");
        let batch_size = self.take_optional_int(&mut block, "batch_size");
        let batch_max_bytes = self.take_optional_int(&mut block, "batch_max_bytes");
        let batch_linger_secs = self.take_optional_duration(&mut block, "batch_linger");
        self.reject_unknown_fields(&mut block, PRODUCER_FIELDS, "producer");
        Some(SqsProducer {
            delay_seconds,
            message_attributes,
            trace_header,
            message_group_id,
            message_deduplication_id,
            batch_size,
            batch_max_bytes,
            batch_linger_secs,
        })
    }

    fn lower_sqs_consumer(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<SqsConsumer> {
        let mut block = self.take_optional_block(fields, "consumer")?;
        let visibility_timeout_secs = self.take_optional_duration(&mut block, "visibility_timeout");
        let wait_time_seconds = self.take_optional_int(&mut block, "wait_time_seconds");
        let max_number_of_messages = self.take_optional_int(&mut block, "max_number_of_messages");
        let message_attribute_names =
            self.take_optional_string_list(&mut block, "message_attribute_names");
        let message_system_attribute_names =
            self.take_optional_string_list(&mut block, "message_system_attribute_names");
        let concurrent_receivers = self.take_optional_int(&mut block, "concurrent_receivers");
        self.reject_unknown_fields(&mut block, CONSUMER_FIELDS, "consumer");
        Some(SqsConsumer {
            visibility_timeout_secs,
            wait_time_seconds,
            max_number_of_messages,
            message_attribute_names,
            message_system_attribute_names,
            concurrent_receivers,
        })
    }
}
