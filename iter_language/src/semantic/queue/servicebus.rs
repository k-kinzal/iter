//! `queue servicebus { ... }` lowerer (Azure Service Bus).
//!
//! Accepts both queue and topic+subscription entities, with an auth
//! surface that mirrors the Azure SDK's credential providers (AAD
//! default chain, connection string, SAS, client secret / certificate,
//! managed / workload identity).

use std::collections::BTreeMap;

use super::super::Analyzer;
use crate::ast::{
    ServiceBusAuth, ServiceBusAuthKind, ServiceBusConfig, ServiceBusProxy, ServiceBusReceiver,
    ServiceBusSender, ServiceBusSession, Span,
};
use crate::diagnostic::Diagnostic;
use crate::parser::RawField;

const SERVICEBUS_FIELDS: &[&str] = &[
    "fully_qualified_namespace",
    "entity_kind",
    "queue_name",
    "topic_name",
    "subscription_name",
    "transport",
    "custom_endpoint_address",
    "web_proxy",
    "connection_idle_timeout",
    "identifier",
    "authority_host",
    "auth",
    "sender",
    "receiver",
    "session",
    "retry",
    "dlq",
];

const SERVICEBUS_PROXY_FIELDS: &[&str] = &["url", "username", "password"];

const SERVICEBUS_SENDER_FIELDS: &[&str] = &[
    "message_id",
    "correlation_id",
    "content_type",
    "subject",
    "reply_to",
    "reply_to_session_id",
    "time_to_live",
    "scheduled_enqueue_time",
    "partition_key_strategy",
    "session_id_strategy",
    "application_properties",
    "batch_size",
    "batch_max_bytes",
    "batch_linger",
    "retry",
];

const SERVICEBUS_RECEIVER_FIELDS: &[&str] = &[
    "receive_mode",
    "prefetch_count",
    "sub_queue",
    "identifier",
    "max_wait_time",
    "max_messages",
    "max_auto_lock_renewal_duration",
    "on_handler_error",
    "dead_letter_reason_template",
    "dead_letter_description_template",
    "retry",
];

const SERVICEBUS_SESSION_FIELDS: &[&str] = &["mode", "session_id", "session_idle_timeout"];

const SB_AUTH_DEFAULT: &[&str] = &["kind"];
const SB_AUTH_CONNSTR: &[&str] = &["kind", "connection_string"];
const SB_AUTH_SAS: &[&str] = &["kind", "sas_token"];
const SB_AUTH_SECRET: &[&str] = &["kind", "tenant_id", "client_id", "client_secret"];
const SB_AUTH_CERT: &[&str] = &[
    "kind",
    "tenant_id",
    "client_id",
    "cert_path",
    "cert_password",
];
const SB_AUTH_MI: &[&str] = &["kind", "client_id"];
const SB_AUTH_WI: &[&str] = &["kind", "tenant_id", "client_id", "token_file"];

impl Analyzer {
    pub(super) fn lower_servicebus(
        &mut self,
        body: BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> ServiceBusConfig {
        let mut fields = body;
        let fully_qualified_namespace =
            self.take_optional_string(&mut fields, "fully_qualified_namespace");
        let entity_kind = self.take_optional_string(&mut fields, "entity_kind");
        let queue_name = self.take_optional_string(&mut fields, "queue_name");
        let topic_name = self.take_optional_string(&mut fields, "topic_name");
        let subscription_name = self.take_optional_string(&mut fields, "subscription_name");
        let transport = self.take_optional_string(&mut fields, "transport");
        let custom_endpoint_address =
            self.take_optional_string(&mut fields, "custom_endpoint_address");
        let web_proxy = self.lower_servicebus_proxy(&mut fields);
        let connection_idle_timeout_secs =
            self.take_optional_duration(&mut fields, "connection_idle_timeout");
        let identifier = self.take_optional_string(&mut fields, "identifier");
        let authority_host = self.take_optional_string(&mut fields, "authority_host");
        let auth = self.lower_servicebus_auth(&mut fields, kind_span);
        let sender = self.lower_servicebus_sender(&mut fields);
        let receiver = self.lower_servicebus_receiver(&mut fields);
        let session = self.lower_servicebus_session(&mut fields, kind_span);
        let retry = self.lower_retry_policy(&mut fields, "retry");
        let dlq = self.lower_dlq_policy(&mut fields, "dlq", kind_span);

        self.reject_unknown_fields(&mut fields, SERVICEBUS_FIELDS, "queue servicebus");

        // Mutual exclusion: queue_name vs topic_name+subscription_name.
        match entity_kind.as_deref() {
            Some("queue") => {
                if queue_name.is_none() {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "queue servicebus: `entity_kind = \"queue\"` requires `queue_name`",
                    ));
                }
            }
            Some("subscription") => {
                if topic_name.is_none() || subscription_name.is_none() {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "queue servicebus: `entity_kind = \"subscription\"` requires `topic_name` and `subscription_name`",
                    ));
                }
            }
            Some(other) => {
                self.errors.push(
                    Diagnostic::error(kind_span.clone(), format!("unknown entity_kind `{other}`"))
                        .with_hint("valid kinds: queue, subscription"),
                );
            }
            None => {
                self.errors.push(
                    Diagnostic::error(kind_span.clone(), "queue servicebus requires `entity_kind`")
                        .with_hint(
                            "set `entity_kind = \"queue\"` or `entity_kind = \"subscription\"`",
                        ),
                );
            }
        }

        // web_proxy is only valid with transport = amqp_websockets.
        if web_proxy.is_some() {
            match transport.as_deref() {
                Some("amqp_websockets") => {}
                _ => {
                    self.errors.push(Diagnostic::error(
                        kind_span.clone(),
                        "queue servicebus: `web_proxy` requires `transport = \"amqp_websockets\"`",
                    ));
                }
            }
        }

        ServiceBusConfig {
            fully_qualified_namespace,
            entity_kind,
            queue_name,
            topic_name,
            subscription_name,
            transport,
            custom_endpoint_address,
            web_proxy,
            connection_idle_timeout_secs,
            identifier,
            authority_host,
            auth,
            sender,
            receiver,
            session,
            retry,
            dlq,
        }
    }

    fn lower_servicebus_proxy(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<ServiceBusProxy> {
        let mut block = self.take_optional_block(fields, "web_proxy")?;
        let Some(url) = self.take_optional_string(&mut block, "url") else {
            let span = block
                .values()
                .next()
                .map_or_else(Span::default, |f| f.name.span.clone());
            self.errors
                .push(Diagnostic::error(span, "web_proxy requires `url`"));
            return None;
        };
        let username = self.take_optional_string(&mut block, "username");
        let password = self.take_optional_secret(&mut block, "password");
        self.reject_unknown_fields(&mut block, SERVICEBUS_PROXY_FIELDS, "servicebus web_proxy");
        Some(ServiceBusProxy {
            url,
            username,
            password,
        })
    }

    fn lower_servicebus_auth(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuth> {
        let mut block = self.take_optional_block(fields, "auth")?;
        let Some(kind) = self.take_optional_string(&mut block, "kind") else {
            self.errors.push(
                Diagnostic::error(kind_span.clone(), "auth block requires `kind = \"...\"`")
                    .with_hint(
                        "valid kinds: aad_default, connection_string, shared_access_signature, aad_client_secret, aad_client_certificate, aad_managed_identity, aad_workload_identity",
                    ),
            );
            return None;
        };
        let lowered = match kind.as_str() {
            "aad_default" => {
                self.reject_unknown_fields(&mut block, SB_AUTH_DEFAULT, "auth aad_default");
                Some(ServiceBusAuthKind::AadDefault)
            }
            "connection_string" => self.lower_sb_auth_connection_string(&mut block, kind_span),
            "shared_access_signature" => self.lower_sb_auth_sas(&mut block, kind_span),
            "aad_client_secret" => self.lower_sb_auth_client_secret(&mut block, kind_span),
            "aad_client_certificate" => self.lower_sb_auth_client_cert(&mut block, kind_span),
            "aad_managed_identity" => {
                let client_id = self.take_optional_string(&mut block, "client_id");
                self.reject_unknown_fields(&mut block, SB_AUTH_MI, "auth aad_managed_identity");
                Some(ServiceBusAuthKind::AadManagedIdentity { client_id })
            }
            "aad_workload_identity" => self.lower_sb_auth_workload(&mut block, kind_span),
            other => {
                self.errors.push(
                    Diagnostic::error(kind_span.clone(), format!("unknown auth kind `{other}`"))
                        .with_hint(
                            "valid kinds: aad_default, connection_string, shared_access_signature, aad_client_secret, aad_client_certificate, aad_managed_identity, aad_workload_identity",
                        ),
                );
                None
            }
        };
        lowered.map(|kind| ServiceBusAuth { kind })
    }

    fn lower_sb_auth_connection_string(
        &mut self,
        block: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuthKind> {
        let value = self.take_required_secret(
            block,
            "connection_string",
            kind_span,
            "auth connection_string",
        );
        self.reject_unknown_fields(block, SB_AUTH_CONNSTR, "auth connection_string");
        Some(ServiceBusAuthKind::ConnectionString { value: value? })
    }

    fn lower_sb_auth_sas(
        &mut self,
        block: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuthKind> {
        let sas_token = self.take_required_secret(
            block,
            "sas_token",
            kind_span,
            "auth shared_access_signature",
        );
        self.reject_unknown_fields(block, SB_AUTH_SAS, "auth shared_access_signature");
        Some(ServiceBusAuthKind::SharedAccessSignature {
            sas_token: sas_token?,
        })
    }

    fn lower_sb_auth_client_secret(
        &mut self,
        block: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuthKind> {
        let tenant_id =
            self.take_required_string(block, "tenant_id", kind_span, "auth aad_client_secret");
        let client_id =
            self.take_required_string(block, "client_id", kind_span, "auth aad_client_secret");
        let client_secret =
            self.take_required_secret(block, "client_secret", kind_span, "auth aad_client_secret");
        self.reject_unknown_fields(block, SB_AUTH_SECRET, "auth aad_client_secret");
        Some(ServiceBusAuthKind::AadClientSecret {
            tenant_id: tenant_id?,
            client_id: client_id?,
            client_secret: client_secret?,
        })
    }

    fn lower_sb_auth_client_cert(
        &mut self,
        block: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuthKind> {
        let tenant_id =
            self.take_required_string(block, "tenant_id", kind_span, "auth aad_client_certificate");
        let client_id =
            self.take_required_string(block, "client_id", kind_span, "auth aad_client_certificate");
        let cert_path =
            self.take_required_string(block, "cert_path", kind_span, "auth aad_client_certificate");
        let cert_password = self.take_optional_secret(block, "cert_password");
        self.reject_unknown_fields(block, SB_AUTH_CERT, "auth aad_client_certificate");
        Some(ServiceBusAuthKind::AadClientCertificate {
            tenant_id: tenant_id?,
            client_id: client_id?,
            cert_path: cert_path?,
            cert_password,
        })
    }

    fn lower_sb_auth_workload(
        &mut self,
        block: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusAuthKind> {
        let tenant_id =
            self.take_required_string(block, "tenant_id", kind_span, "auth aad_workload_identity");
        let client_id =
            self.take_required_string(block, "client_id", kind_span, "auth aad_workload_identity");
        let token_file =
            self.take_required_string(block, "token_file", kind_span, "auth aad_workload_identity");
        self.reject_unknown_fields(block, SB_AUTH_WI, "auth aad_workload_identity");
        Some(ServiceBusAuthKind::AadWorkloadIdentity {
            tenant_id: tenant_id?,
            client_id: client_id?,
            token_file: token_file?,
        })
    }

    fn lower_servicebus_sender(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<ServiceBusSender> {
        let mut block = self.take_optional_block(fields, "sender")?;
        let message_id = self.take_optional_templated_string(&mut block, "message_id");
        let correlation_id = self.take_optional_templated_string(&mut block, "correlation_id");
        let content_type = self.take_optional_string(&mut block, "content_type");
        let subject = self.take_optional_string(&mut block, "subject");
        let reply_to = self.take_optional_string(&mut block, "reply_to");
        let reply_to_session_id = self.take_optional_string(&mut block, "reply_to_session_id");
        let time_to_live_secs = self.take_optional_duration(&mut block, "time_to_live");
        let scheduled_enqueue_time =
            self.take_optional_string(&mut block, "scheduled_enqueue_time");
        let partition_key_strategy =
            self.take_optional_templated_string(&mut block, "partition_key_strategy");
        let session_id_strategy =
            self.take_optional_templated_string(&mut block, "session_id_strategy");
        let application_properties =
            self.take_optional_string_string_block(&mut block, "application_properties");
        let batch_size = self.take_optional_int(&mut block, "batch_size");
        let batch_max_bytes = self.take_optional_int(&mut block, "batch_max_bytes");
        let batch_linger_secs = self.take_optional_duration(&mut block, "batch_linger");
        let retry = self.lower_retry_policy(&mut block, "retry");
        self.reject_unknown_fields(&mut block, SERVICEBUS_SENDER_FIELDS, "servicebus sender");
        Some(ServiceBusSender {
            message_id,
            correlation_id,
            content_type,
            subject,
            reply_to,
            reply_to_session_id,
            time_to_live_secs,
            scheduled_enqueue_time,
            partition_key_strategy,
            session_id_strategy,
            application_properties,
            batch_size,
            batch_max_bytes,
            batch_linger_secs,
            retry,
        })
    }

    fn lower_servicebus_receiver(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<ServiceBusReceiver> {
        let mut block = self.take_optional_block(fields, "receiver")?;
        let receive_mode = self.take_optional_string(&mut block, "receive_mode");
        let prefetch_count = self.take_optional_int(&mut block, "prefetch_count");
        let sub_queue = self.take_optional_string(&mut block, "sub_queue");
        let identifier = self.take_optional_string(&mut block, "identifier");
        let max_wait_time_secs = self.take_optional_duration(&mut block, "max_wait_time");
        let max_messages = self.take_optional_int(&mut block, "max_messages");
        let max_auto_lock_renewal_duration_secs =
            self.take_optional_duration(&mut block, "max_auto_lock_renewal_duration");
        let on_handler_error = self.take_optional_string(&mut block, "on_handler_error");
        let dead_letter_reason_template =
            self.take_optional_string(&mut block, "dead_letter_reason_template");
        let dead_letter_description_template =
            self.take_optional_string(&mut block, "dead_letter_description_template");
        let retry = self.lower_retry_policy(&mut block, "retry");
        self.reject_unknown_fields(
            &mut block,
            SERVICEBUS_RECEIVER_FIELDS,
            "servicebus receiver",
        );
        Some(ServiceBusReceiver {
            receive_mode,
            prefetch_count,
            sub_queue,
            identifier,
            max_wait_time_secs,
            max_messages,
            max_auto_lock_renewal_duration_secs,
            on_handler_error,
            dead_letter_reason_template,
            dead_letter_description_template,
            retry,
        })
    }

    fn lower_servicebus_session(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<ServiceBusSession> {
        let mut block = self.take_optional_block(fields, "session")?;
        let mode = self.take_optional_string(&mut block, "mode");
        let session_id = self.take_optional_string(&mut block, "session_id");
        let session_idle_timeout_secs =
            self.take_optional_duration(&mut block, "session_idle_timeout");
        self.reject_unknown_fields(&mut block, SERVICEBUS_SESSION_FIELDS, "servicebus session");
        if matches!(mode.as_deref(), Some("accept_specific")) && session_id.is_none() {
            self.errors.push(Diagnostic::error(
                kind_span.clone(),
                "session mode `accept_specific` requires `session_id`",
            ));
        }
        Some(ServiceBusSession {
            mode,
            session_id,
            session_idle_timeout_secs,
        })
    }
}
