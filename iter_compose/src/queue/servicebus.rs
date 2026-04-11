//! Translate Service Bus configuration from the language AST into `iter_core` types.

use iter_core::queue::azure::{
    ServiceBusCredentials as CoreServiceBusCredentials, ServiceBusEntity as CoreServiceBusEntity,
    ServiceBusProxyConfig as CoreServiceBusProxy, ServiceBusQueueConfig,
    ServiceBusReceiverConfig as CoreServiceBusReceiver,
    ServiceBusSenderConfig as CoreServiceBusSender,
    ServiceBusSessionConfig as CoreServiceBusSession,
};
use iter_language::{
    ServiceBusAuth, ServiceBusAuthKind, ServiceBusConfig, ServiceBusProxy, ServiceBusReceiver,
    ServiceBusSender, ServiceBusSession,
};

use super::{
    QueueBuildError, opt_u32, secs_to_duration, translate_dlq, translate_retry, translate_template,
};
use crate::secrets::resolve_secret;

pub(super) fn build_servicebus_config(
    cfg: &ServiceBusConfig,
) -> Result<ServiceBusQueueConfig, QueueBuildError> {
    let entity_kind = cfg.entity_kind.as_deref().unwrap_or("");
    let entity = match entity_kind {
        "queue" => {
            let name = cfg.queue_name.clone().ok_or_else(|| {
                QueueBuildError::invalid("entity_kind = \"queue\" requires `queue_name`")
            })?;
            CoreServiceBusEntity::Queue { name }
        }
        "subscription" => {
            let topic = cfg.topic_name.clone().ok_or_else(|| {
                QueueBuildError::invalid("entity_kind = \"subscription\" requires `topic_name`")
            })?;
            let subscription = cfg.subscription_name.clone().ok_or_else(|| {
                QueueBuildError::invalid(
                    "entity_kind = \"subscription\" requires `subscription_name`",
                )
            })?;
            CoreServiceBusEntity::Subscription {
                topic,
                subscription,
            }
        }
        other => {
            return Err(QueueBuildError::invalid(format!(
                "unknown entity_kind `{other}`; expected one of queue | subscription"
            )));
        }
    };

    let credentials = cfg
        .auth
        .as_ref()
        .map(translate_servicebus_auth)
        .transpose()?;
    let web_proxy = cfg
        .web_proxy
        .as_ref()
        .map(translate_servicebus_proxy)
        .transpose()?;
    let sender = cfg
        .sender
        .as_ref()
        .map(translate_servicebus_sender)
        .transpose()?;
    let receiver = cfg
        .receiver
        .as_ref()
        .map(translate_servicebus_receiver)
        .transpose()?;
    let session = cfg.session.as_ref().map(translate_servicebus_session);
    let retry = cfg.retry.as_ref().map(translate_retry).transpose()?;
    let dlq = cfg.dlq.as_ref().map(translate_dlq).transpose()?;

    Ok(ServiceBusQueueConfig {
        fully_qualified_namespace: cfg.fully_qualified_namespace.clone(),
        entity,
        transport: cfg.transport.clone(),
        custom_endpoint_address: cfg.custom_endpoint_address.clone(),
        web_proxy,
        connection_idle_timeout: cfg.connection_idle_timeout_secs.map(secs_to_duration),
        identifier: cfg.identifier.clone(),
        authority_host: cfg.authority_host.clone(),
        credentials,
        sender,
        receiver,
        session,
        retry,
        dlq,
    })
}

fn translate_servicebus_auth(
    auth: &ServiceBusAuth,
) -> Result<CoreServiceBusCredentials, QueueBuildError> {
    Ok(match &auth.kind {
        ServiceBusAuthKind::AadDefault => CoreServiceBusCredentials::AadDefault,
        ServiceBusAuthKind::ConnectionString { value } => {
            CoreServiceBusCredentials::ConnectionString {
                value: resolve_secret(value)
                    .map_err(|s| QueueBuildError::secret("auth.connection_string", s))?,
            }
        }
        ServiceBusAuthKind::SharedAccessSignature { sas_token } => {
            CoreServiceBusCredentials::SharedAccessSignature {
                sas_token: resolve_secret(sas_token)
                    .map_err(|s| QueueBuildError::secret("auth.sas_token", s))?,
            }
        }
        ServiceBusAuthKind::AadClientSecret {
            tenant_id,
            client_id,
            client_secret,
        } => CoreServiceBusCredentials::AadClientSecret {
            tenant_id: tenant_id.clone(),
            client_id: client_id.clone(),
            client_secret: resolve_secret(client_secret)
                .map_err(|s| QueueBuildError::secret("auth.client_secret", s))?,
        },
        ServiceBusAuthKind::AadClientCertificate {
            tenant_id,
            client_id,
            cert_path,
            cert_password,
        } => CoreServiceBusCredentials::AadClientCertificate {
            tenant_id: tenant_id.clone(),
            client_id: client_id.clone(),
            cert_path: cert_path.clone(),
            cert_password: cert_password
                .as_ref()
                .map(resolve_secret)
                .transpose()
                .map_err(|s| QueueBuildError::secret("auth.cert_password", s))?,
        },
        ServiceBusAuthKind::AadManagedIdentity { client_id } => {
            CoreServiceBusCredentials::AadManagedIdentity {
                client_id: client_id.clone(),
            }
        }
        ServiceBusAuthKind::AadWorkloadIdentity {
            tenant_id,
            client_id,
            token_file,
        } => CoreServiceBusCredentials::AadWorkloadIdentity {
            tenant_id: tenant_id.clone(),
            client_id: client_id.clone(),
            token_file: token_file.clone(),
        },
    })
}

fn translate_servicebus_proxy(p: &ServiceBusProxy) -> Result<CoreServiceBusProxy, QueueBuildError> {
    Ok(CoreServiceBusProxy {
        url: p.url.clone(),
        username: p.username.clone(),
        password: p
            .password
            .as_ref()
            .map(resolve_secret)
            .transpose()
            .map_err(|s| QueueBuildError::secret("web_proxy.password", s))?,
    })
}

fn translate_servicebus_sender(
    s: &ServiceBusSender,
) -> Result<CoreServiceBusSender, QueueBuildError> {
    let retry = s.retry.as_ref().map(translate_retry).transpose()?;
    Ok(CoreServiceBusSender {
        message_id: s.message_id.as_ref().map(translate_template),
        correlation_id: s.correlation_id.as_ref().map(translate_template),
        content_type: s.content_type.clone(),
        subject: s.subject.clone(),
        reply_to: s.reply_to.clone(),
        reply_to_session_id: s.reply_to_session_id.clone(),
        time_to_live: s.time_to_live_secs.map(secs_to_duration),
        scheduled_enqueue_time: s.scheduled_enqueue_time.clone(),
        partition_key_strategy: s.partition_key_strategy.as_ref().map(translate_template),
        session_id_strategy: s.session_id_strategy.as_ref().map(translate_template),
        application_properties: s.application_properties.clone(),
        batch_size: opt_u32(s.batch_size),
        batch_max_bytes: opt_u32(s.batch_max_bytes),
        batch_linger: s.batch_linger_secs.map(secs_to_duration),
        retry,
    })
}

fn translate_servicebus_receiver(
    r: &ServiceBusReceiver,
) -> Result<CoreServiceBusReceiver, QueueBuildError> {
    let retry = r.retry.as_ref().map(translate_retry).transpose()?;
    Ok(CoreServiceBusReceiver {
        receive_mode: r.receive_mode.clone(),
        prefetch_count: opt_u32(r.prefetch_count),
        sub_queue: r.sub_queue.clone(),
        identifier: r.identifier.clone(),
        max_wait_time: r.max_wait_time_secs.map(secs_to_duration),
        max_messages: opt_u32(r.max_messages),
        max_auto_lock_renewal_duration: r.max_auto_lock_renewal_duration_secs.map(secs_to_duration),
        on_handler_error: r.on_handler_error.clone(),
        dead_letter_reason_template: r.dead_letter_reason_template.clone(),
        dead_letter_description_template: r.dead_letter_description_template.clone(),
        retry,
    })
}

fn translate_servicebus_session(s: &ServiceBusSession) -> CoreServiceBusSession {
    CoreServiceBusSession {
        mode: s.mode.clone(),
        session_id: s.session_id.clone(),
        session_idle_timeout: s.session_idle_timeout_secs.map(secs_to_duration),
    }
}
