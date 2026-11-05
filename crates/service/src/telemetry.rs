// SPDX-License-Identifier: AGPL-3.0-or-later

//! Structured logging with `tracing`, and error reporting to Sentry.
//!
//! Sentry is off unless `HERON_SENTRY_DSN` is set. When it is on, errors
//! logged with `tracing::error!` and panics are reported as Sentry events,
//! with the warnings before them as breadcrumbs. Informational logs are not
//! sent, and fields that may identify people (`user`, `username`…) are
//! redacted from whatever is. Nothing is sent from the audio thread, which
//! never logs.
//!
//! Bad settings (an invalid DSN, a malformed sample rate) are logged, never
//! fatal: error reporting must not keep a service, the engine above all,
//! from running.

use std::collections::BTreeMap;

use sentry::integrations::tracing::{BreadcrumbLayer, ErrorLayer, SpanLayer};
use tracing_subscriber::filter::{LevelFilter, filter_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::Error;

/// Log output format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    /// Human-readable lines, for local development.
    #[default]
    Text,
    /// One JSON object per line, for containers and log collectors.
    Json,
}

/// Logging and error reporting options shared by every binary.
#[derive(Clone, Debug, clap::Args)]
pub struct LogArgs {
    /// Log output format. The level is set with `RUST_LOG` (default `info`).
    #[arg(long, env = "HERON_LOG_FORMAT", value_enum, default_value_t = LogFormat::Text, global = true)]
    pub log_format: LogFormat,

    /// Sentry DSN: errors and panics are reported to this Sentry project.
    /// Off when empty.
    #[arg(long, env = "HERON_SENTRY_DSN", global = true, hide_env_values = true)]
    pub sentry_dsn: Option<String>,

    /// Environment reported to Sentry (production, staging…).
    #[arg(
        long,
        env = "HERON_SENTRY_ENVIRONMENT",
        default_value = "production",
        global = true
    )]
    pub sentry_environment: String,

    /// Share of operations traced for Sentry's performance monitoring, from 0
    /// (none) to 1 (all). Read leniently: an invalid value disables tracing.
    #[arg(
        long,
        env = "HERON_SENTRY_TRACES_SAMPLE_RATE",
        default_value = "0",
        global = true
    )]
    pub sentry_traces_sample_rate: String,
}

/// Keep it alive until the process exits: dropping it flushes the events
/// not sent yet.
#[must_use = "dropping it stops error reporting"]
pub struct Telemetry {
    _sentry: Option<sentry::ClientInitGuard>,
}

/// What Sentry gets from `tracing`: spans at INFO and above as traces, WARN
/// events as breadcrumbs, ERROR events as error events.
fn sentry_layers<S>() -> Vec<Box<dyn Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    vec![
        SpanLayer::new().with_filter(LevelFilter::INFO).boxed(),
        BreadcrumbLayer::new()
            .with_filter(filter_fn(|metadata| {
                *metadata.level() == tracing::Level::WARN
            }))
            .boxed(),
        ErrorLayer::new().with_filter(LevelFilter::ERROR).boxed(),
    ]
}

/// Installs the global `tracing` subscriber, and Sentry when configured.
pub fn init_tracing(args: &LogArgs, service: &'static str) -> Result<Telemetry, Error> {
    // Problems with the settings, logged once logging works.
    let mut problems = Vec::new();
    let sentry = args
        .sentry_dsn
        .as_deref()
        .map(str::trim)
        .filter(|dsn| !dsn.is_empty())
        .and_then(|dsn| match dsn.parse::<sentry::types::Dsn>() {
            Ok(dsn) => Some(dsn),
            Err(error) => {
                problems.push(format!(
                    "invalid HERON_SENTRY_DSN ({error}): errors are not reported to Sentry"
                ));
                None
            }
        })
        .map(|dsn| {
            let mut options = sentry::ClientOptions::new()
                .release(format!("{service}@{}", env!("CARGO_PKG_VERSION")))
                .environment(args.sentry_environment.clone())
                // Never IP addresses or request bodies.
                .send_default_pii(false)
                .before_breadcrumb(|mut breadcrumb| {
                    redact(&mut breadcrumb.data);
                    Some(breadcrumb)
                })
                .before_send(|mut event| {
                    redact_event(&mut event);
                    Some(event)
                });
            options.dsn = Some(dsn);
            match sample_rate(&args.sentry_traces_sample_rate) {
                Some(rate) if rate > 0.0 => options = options.traces_sample_rate(rate),
                Some(_) => {}
                None => problems.push(format!(
                    "invalid HERON_SENTRY_TRACES_SAMPLE_RATE {:?} (0 to 1): no tracing",
                    args.sentry_traces_sample_rate
                )),
            }
            let guard = sentry::init(options);
            sentry::configure_scope(|scope| scope.set_tag("service", service));
            guard
        });

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let output = match args.log_format {
        LogFormat::Text => tracing_subscriber::fmt::layer().boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .boxed(),
    };
    let enabled = sentry.is_some();
    let init = tracing_subscriber::registry()
        .with(filter)
        .with(output)
        .with(enabled.then(sentry_layers))
        .try_init();
    init.map_err(|error| Error::Tracing(error.to_string()))?;
    if sentry.is_some() {
        tracing::info!(environment = %args.sentry_environment, "reporting errors to Sentry");
    }
    for problem in problems {
        tracing::error!("{problem}");
    }
    Ok(Telemetry { _sentry: sentry })
}

/// A share from 0 to 1; `None` when it is not one.
fn sample_rate(text: &str) -> Option<f32> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|rate| (0.0..=1.0).contains(rate))
}

/// Log fields that may identify people, never sent to Sentry.
const PERSONAL_FIELDS: &[&str] = &[
    "user",
    "username",
    "by",
    "email",
    "ip",
    "address",
    "remote_addr",
    "password",
    "token",
    "key",
    "acoustid_key",
];

const REDACTED: &str = "[redacted]";

fn redact(fields: &mut BTreeMap<String, serde_json::Value>) {
    for (name, value) in fields.iter_mut() {
        if PERSONAL_FIELDS.contains(&name.as_str()) {
            *value = REDACTED.into();
        }
    }
}

/// Redacts the fields of a log turned into an event (its contexts), its
/// tags and extra data.
fn redact_event(event: &mut sentry::protocol::Event<'static>) {
    redact(&mut event.extra);
    for context in event.contexts.values_mut() {
        if let sentry::protocol::Context::Other(fields) = context {
            redact(fields);
        }
    }
    for (name, value) in event.tags.iter_mut() {
        if PERSONAL_FIELDS.contains(&name.as_str()) {
            *value = REDACTED.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentry_layers_take_spans_and_events() {
        let subscriber = tracing_subscriber::registry().with(sentry_layers());
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("work");
            let _entered = span.enter();
            tracing::debug!("not kept");
            tracing::warn!("a breadcrumb");
            tracing::error!("an error event");
        });
    }

    #[test]
    fn sample_rates_are_read_leniently() {
        assert_eq!(sample_rate(" 0.25 "), Some(0.25));
        assert_eq!(sample_rate("0"), Some(0.0));
        assert_eq!(sample_rate("10%"), None);
        assert_eq!(sample_rate("2"), None);
        assert_eq!(sample_rate("NaN"), None);
    }

    #[test]
    fn personal_fields_are_redacted() {
        let mut fields: BTreeMap<String, serde_json::Value> = [
            ("username".to_owned(), "alice".into()),
            ("by".to_owned(), "bob".into()),
            ("path".to_owned(), "Artist/Album/01.mp3".into()),
        ]
        .into();
        redact(&mut fields);
        assert_eq!(fields["username"], REDACTED);
        assert_eq!(fields["by"], REDACTED);
        assert_eq!(fields["path"], "Artist/Album/01.mp3");

        let mut event = sentry::protocol::Event::default();
        event.contexts.insert(
            "Rust Tracing Fields".to_owned(),
            sentry::protocol::Context::Other(
                [("user".to_owned(), serde_json::Value::from("carol"))].into(),
            ),
        );
        event.tags.insert("username".to_owned(), "dave".to_owned());
        redact_event(&mut event);
        let sentry::protocol::Context::Other(fields) = &event.contexts["Rust Tracing Fields"]
        else {
            panic!("context changed type");
        };
        assert_eq!(fields["user"], REDACTED);
        assert_eq!(event.tags["username"], REDACTED);
    }
}
