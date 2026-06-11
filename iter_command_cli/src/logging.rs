//! `--log-level` / `--log-format` / `--debug` flag set and the
//! `tracing_subscriber` initialisation it drives.
//!
//! Tracing output is unconditionally routed to stderr — stdout is
//! reserved for the CLI's user-visible contract.

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn directive(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Args)]
pub(crate) struct LoggingArgs {
    /// Logging verbosity.
    #[arg(long = "log-level", value_enum, default_value_t = LogLevel::Info)]
    pub(crate) log_level: LogLevel,

    /// Log output format.
    #[arg(long = "log-format", value_enum, default_value_t = LogFormat::Text)]
    pub(crate) log_format: LogFormat,

    /// Shortcut for `--log-level debug`.
    #[arg(long)]
    pub(crate) debug: bool,
}

impl LoggingArgs {
    pub(crate) fn init(&self) -> iter_tracing::TelemetryGuard {
        use tracing_subscriber::EnvFilter;

        let level = if self.debug {
            "debug"
        } else {
            self.log_level.directive()
        };
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

        // iter_tracing never invents a `service.name`; this binary supplies its
        // own (its `CARGO_BIN_NAME`) when the operator left `OTEL_SERVICE_NAME`
        // unset.
        let otel = iter_tracing::OtelRuntimeConfig::from_env().map(|mut config| {
            if config.service_name.is_none() {
                config.service_name = Some(env!("CARGO_BIN_NAME").to_string());
            }
            config
        });

        iter_tracing::install_stderr_subscriber(
            filter,
            matches!(self.log_format, LogFormat::Json),
            otel,
        )
    }
}
