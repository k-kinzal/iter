use super::event::ErrorStage;

/// Errors emitted by [`super::Runner::run`].
#[derive(Debug, thiserror::Error)]
// All variants name the stage that failed; shared suffix is intentional.
#[allow(clippy::enum_variant_names)]
pub enum RunnerExitError {
    /// A dequeue operation failed and `continue_on_error` was `false`.
    #[error("dequeue failed: {message}")]
    DequeueFailed {
        /// Stringified source error.
        message: String,
        /// Boxed original source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Running tally of event handler errors across the run.
        event_handler_error_count: u32,
        /// Running tally of observer errors across the run.
        observer_error_count: u32,
    },
    /// Prompt rendering failed and `continue_on_error` was `false`.
    #[error("render prompt failed: {message}")]
    RenderPromptFailed {
        /// Stringified source error.
        message: String,
        /// Boxed original source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Running tally of event handler errors across the run.
        event_handler_error_count: u32,
        /// Running tally of observer errors across the run.
        observer_error_count: u32,
    },
    /// Workspace setup failed and `continue_on_error` was `false`.
    #[error("workspace setup failed: {message}")]
    WorkspaceSetupFailed {
        /// Stringified source error.
        message: String,
        /// Boxed original source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Running tally of event handler errors across the run.
        event_handler_error_count: u32,
        /// Running tally of observer errors across the run.
        observer_error_count: u32,
    },
    /// Agent run failed and `continue_on_error` was `false`.
    #[error("agent run failed: {message}")]
    AgentRunFailed {
        /// Stringified source error.
        message: String,
        /// Boxed original source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Running tally of event handler errors across the run.
        event_handler_error_count: u32,
        /// Running tally of observer errors across the run.
        observer_error_count: u32,
    },
    /// Workspace teardown failed and `continue_on_error` was `false`.
    #[error("workspace teardown failed: {message}")]
    WorkspaceTeardownFailed {
        /// Stringified source error.
        message: String,
        /// Boxed original source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        /// Running tally of event handler errors across the run.
        event_handler_error_count: u32,
        /// Running tally of observer errors across the run.
        observer_error_count: u32,
    },
}

impl RunnerExitError {
    /// Return the [`ErrorStage`] label for this error.
    #[must_use]
    pub fn stage(&self) -> ErrorStage {
        match self {
            Self::DequeueFailed { .. } => ErrorStage::Dequeue,
            Self::RenderPromptFailed { .. } => ErrorStage::RenderPrompt,
            Self::WorkspaceSetupFailed { .. } => ErrorStage::WorkspaceSetup,
            Self::AgentRunFailed { .. } => ErrorStage::AgentRun,
            Self::WorkspaceTeardownFailed { .. } => ErrorStage::WorkspaceTeardown,
        }
    }

    pub(super) fn message(&self) -> &str {
        match self {
            Self::DequeueFailed { message, .. }
            | Self::RenderPromptFailed { message, .. }
            | Self::WorkspaceSetupFailed { message, .. }
            | Self::AgentRunFailed { message, .. }
            | Self::WorkspaceTeardownFailed { message, .. } => message,
        }
    }

    pub(super) fn with_counters(
        self,
        event_handler_error_count: u32,
        observer_error_count: u32,
    ) -> Self {
        match self {
            Self::DequeueFailed {
                message, source, ..
            } => Self::DequeueFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::RenderPromptFailed {
                message, source, ..
            } => Self::RenderPromptFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::WorkspaceSetupFailed {
                message, source, ..
            } => Self::WorkspaceSetupFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::AgentRunFailed {
                message, source, ..
            } => Self::AgentRunFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
            Self::WorkspaceTeardownFailed {
                message, source, ..
            } => Self::WorkspaceTeardownFailed {
                message,
                source,
                event_handler_error_count,
                observer_error_count,
            },
        }
    }
}
