#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

mod trigger_util;
pub mod webhook;

pub use webhook::{Subscription, WebhookConfig, WebhookTrigger, WebhookTriggerError};
