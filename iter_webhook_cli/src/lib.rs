#![deny(rust_2018_idioms)]

mod trigger_util;
pub mod webhook;

pub use webhook::{Subscription, WebhookConfig, WebhookTrigger, WebhookTriggerError};
