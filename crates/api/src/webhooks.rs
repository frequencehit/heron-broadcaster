// SPDX-License-Identifier: AGPL-3.0-or-later

//! Webhooks on track changes (`webhooks`), and their deliveries
//! (`webhook_deliveries`). See `docs/api.md#webhooks`.
//!
//! The dispatcher turns each track change from the engine into a delivery
//! per matching webhook: `track.started`, `track.skipped` for a skip and
//! `track.ended` for any other end, FX player tracks only for webhooks that
//! ask for them. The sender posts deliveries as soon as they are queued,
//! signed with the webhook's secret (HMAC-SHA256), and retries those that
//! fail with backoff. Deliveries are rows in the database: retries survive
//! a restart, and the rows are the delivery log, kept for 30 days.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use base64::Engine as _;
use heron_core::PlaySource;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::MySqlPool;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinSet;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::{DeckName, EndReason, LinkEvent, TrackEvent, unix_ms};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};

/// Deliveries sent at the same time.
const MAX_IN_FLIGHT: usize = 8;
/// A claimed delivery is sent again after this long if the API stopped
/// while sending it.
const CLAIM: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest the sender sleeps between two looks at the queue.
const IDLE: Duration = Duration::from_secs(60);
const PRUNE_INTERVAL: Duration = Duration::from_secs(3_600);
/// How long deliveries stay in the log.
const KEEP_DAYS: u32 = 30;
/// What is kept of the receiver's answer.
const RESPONSE_CHARS: usize = 1_024;
const ERROR_CHARS: usize = 500;
const MAX_TEMPLATE_BYTES: usize = 65_536;

/// What a delivery reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum WebhookEvent {
    #[serde(rename = "track.started")]
    TrackStarted,
    /// A track played to its end (or its transition), or an FX that was
    /// stopped or replaced.
    #[serde(rename = "track.ended")]
    TrackEnded,
    #[serde(rename = "track.skipped")]
    TrackSkipped,
    /// Sent by `POST /webhooks/{id}/test`.
    #[serde(rename = "ping")]
    Ping,
}

impl WebhookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TrackStarted => "track.started",
            Self::TrackEnded => "track.ended",
            Self::TrackSkipped => "track.skipped",
            Self::Ping => "ping",
        }
    }

    /// The event a track change sends.
    pub fn of(event: &TrackEvent) -> Self {
        match event {
            TrackEvent::Started { .. } => Self::TrackStarted,
            TrackEvent::Ended {
                reason: EndReason::Skipped,
                ..
            } => Self::TrackSkipped,
            TrackEvent::Ended { .. } => Self::TrackEnded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Waiting for its first or next attempt.
    Pending,
    Delivered,
    /// Every attempt failed.
    Failed,
}

impl DeliveryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "delivered" => Self::Delivered,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// The track of a delivery.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PayloadTrack {
    /// Missing for files outside the library.
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub duration_ms: u32,
    pub deck: DeckName,
    pub source: PlaySource,
}

/// The default body of a delivery.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Payload {
    pub event: WebhookEvent,
    pub delivery_id: u64,
    pub at_unix_ms: u64,
    /// Missing for a ping.
    pub track: Option<PayloadTrack>,
    /// How long the track played, when it ended.
    pub played_ms: Option<u64>,
    /// Why it ended: `finished`, `skipped`, `stopped` or `replaced`.
    pub reason: Option<EndReason>,
}

impl Payload {
    fn of(event: &TrackEvent) -> Self {
        let (deck, entry, at_unix_ms, played_ms, reason) = match event {
            TrackEvent::Started {
                deck,
                entry,
                at_unix_ms,
            } => (*deck, entry, *at_unix_ms, None, None),
            TrackEvent::Ended {
                deck,
                entry,
                at_unix_ms,
                played_ms,
                reason,
            } => (*deck, entry, *at_unix_ms, Some(*played_ms), Some(*reason)),
        };
        let track = &entry.track;
        Self {
            event: WebhookEvent::of(event),
            delivery_id: 0,
            at_unix_ms,
            track: Some(PayloadTrack {
                track_id: track.track_id,
                artist: track.artist.clone(),
                title: track.title.clone(),
                album: track.album.clone(),
                duration_ms: track.duration_ms,
                deck,
                source: track.source,
            }),
            played_ms,
            reason,
        }
    }

    fn ping() -> Self {
        Self {
            event: WebhookEvent::Ping,
            delivery_id: 0,
            at_unix_ms: unix_ms(SystemTime::now()),
            track: None,
            played_ms: None,
            reason: None,
        }
    }

    /// The value of a template placeholder; `None` for unknown names.
    fn field(&self, name: &str) -> Option<String> {
        let track = self.track.as_ref();
        let text = |value: Option<&String>| value.cloned().unwrap_or_default();
        let enum_text = |value: serde_json::Value| value.as_str().unwrap_or_default().to_owned();
        Some(match name {
            "event" => self.event.as_str().to_owned(),
            "delivery_id" => self.delivery_id.to_string(),
            "at_unix_ms" => self.at_unix_ms.to_string(),
            "track_id" => track
                .and_then(|track| track.track_id)
                .map(|id| id.to_string())
                .unwrap_or_default(),
            "artist" => text(track.and_then(|track| track.artist.as_ref())),
            "title" => text(track.and_then(|track| track.title.as_ref())),
            "album" => text(track.and_then(|track| track.album.as_ref())),
            "duration_ms" => track
                .map(|track| track.duration_ms.to_string())
                .unwrap_or_default(),
            "deck" => track
                .map(|track| track.deck.as_str().to_owned())
                .unwrap_or_default(),
            "source" => track
                .and_then(|track| serde_json::to_value(track.source).ok())
                .map(enum_text)
                .unwrap_or_default(),
            "played_ms" => self
                .played_ms
                .map(|played| played.to_string())
                .unwrap_or_default(),
            "reason" => self
                .reason
                .map(|reason| reason.as_str().to_owned())
                .unwrap_or_default(),
            _ => return None,
        })
    }
}

/// The placeholders a body template can use.
pub const PLACEHOLDERS: [&str; 12] = [
    "event",
    "delivery_id",
    "at_unix_ms",
    "track_id",
    "artist",
    "title",
    "album",
    "duration_ms",
    "deck",
    "source",
    "played_ms",
    "reason",
];

/// Splits a template into text and `{{placeholder}}` names (spaces inside
/// the braces allowed). An unclosed `{{` is text.
fn parts(template: &str) -> Vec<Result<&str, &str>> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(length) = rest[start + 2..].find("}}") else {
            break;
        };
        if start > 0 {
            parts.push(Ok(&rest[..start]));
        }
        parts.push(Err(rest[start + 2..start + 2 + length].trim()));
        rest = &rest[start + 2 + length + 2..];
    }
    if !rest.is_empty() {
        parts.push(Ok(rest));
    }
    parts
}

/// Refuses templates with unknown placeholders.
pub fn check_template(template: &str) -> Result<(), String> {
    if template.len() > MAX_TEMPLATE_BYTES {
        return Err("the body template is longer than 64 KiB".to_owned());
    }
    let unknown: Vec<&str> = parts(template)
        .into_iter()
        .filter_map(Result::err)
        .filter(|name| !PLACEHOLDERS.contains(name))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unknown placeholders in the body template: {}; use {}",
            unknown.join(", "),
            PLACEHOLDERS.join(", ")
        ))
    }
}

fn is_json(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("json")
}

/// Fills a template. Values are escaped as JSON string content when the
/// body is JSON (`"{{title}}"` stays valid JSON); missing values are empty.
pub fn render(template: &str, content_type: &str, payload: &Payload) -> String {
    let json = is_json(content_type);
    let mut body = String::with_capacity(template.len());
    for part in parts(template) {
        match part {
            Ok(text) => body.push_str(text),
            Err(name) => {
                let value = payload.field(name).unwrap_or_default();
                if json {
                    let quoted = serde_json::Value::String(value).to_string();
                    body.push_str(&quoted[1..quoted.len() - 1]);
                } else {
                    body.push_str(&value);
                }
            }
        }
    }
    body
}

fn body(template: Option<&str>, content_type: &str, payload: &Payload) -> String {
    match template {
        Some(template) => render(template, content_type, payload),
        None => serde_json::to_string(payload).unwrap_or_default(),
    }
}

fn hmac_hex(key: &[u8], data: &[u8]) -> String {
    // HMAC takes keys of any length: this never fails.
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(data);
    let mut hex = String::with_capacity(64);
    for byte in mac.finalize().into_bytes() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `X-Heron-Signature`: `sha256=` and the HMAC-SHA256 of
/// `"{timestamp}.{body}"` with the webhook's secret, in hex.
pub fn signature(secret: &str, timestamp: u64, body: &str) -> String {
    format!(
        "sha256={}",
        hmac_hex(secret.as_bytes(), format!("{timestamp}.{body}").as_bytes())
    )
}

fn new_secret() -> ApiResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| ApiError::Internal(format!("no randomness: {error}")))?;
    Ok(format!(
        "whsec_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

/// The waits between attempts: after the last one fails, the delivery has
/// failed.
#[derive(Clone, Debug)]
pub struct Backoff(pub Vec<Duration>);

impl Default for Backoff {
    /// 8 attempts over about 5 hours.
    fn default() -> Self {
        Self(
            [10, 30, 120, 600, 1_800, 3_600, 10_800]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
        )
    }
}

impl Backoff {
    /// The wait after `attempts` failed attempts, `None` when that was the
    /// last one.
    pub fn after(&self, attempts: u16) -> Option<Duration> {
        self.0.get(usize::from(attempts).checked_sub(1)?).copied()
    }
}

fn truncate(mut text: String, chars: usize) -> String {
    if let Some((index, _)) = text.char_indices().nth(chars) {
        text.truncate(index);
    }
    text
}

/// The error and its causes: reqwest's own message says little.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(text, ": {cause}");
        source = cause.source();
    }
    text
}

pub struct Webhooks {
    pool: MySqlPool,
    client: reqwest::Client,
    backoff: Backoff,
    /// Wakes the sender when a delivery is queued.
    wake: Notify,
}

type DueRow = (String, String, String, String, String, u16, bool);

impl Webhooks {
    pub fn new(pool: MySqlPool) -> reqwest::Result<Arc<Self>> {
        Self::with_backoff(pool, Backoff::default())
    }

    pub fn with_backoff(pool: MySqlPool, backoff: Backoff) -> reqwest::Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("heron-broadcaster/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Arc::new(Self {
            pool,
            client,
            backoff,
            wake: Notify::new(),
        }))
    }

    /// Queues deliveries for the engine's track changes.
    pub async fn dispatch(self: Arc<Self>, mut events: broadcast::Receiver<LinkEvent>) {
        loop {
            let event = match events.recv().await {
                Ok(LinkEvent::Track(event)) => event,
                // Plays missed while the API was away are not reported.
                Ok(LinkEvent::Connected(_)) => continue,
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "webhooks fell behind: track changes were not sent");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if let Err(error) = self.queue_event(&event).await {
                tracing::error!(%error, "cannot queue webhook deliveries");
            }
        }
    }

    async fn queue_event(&self, event: &TrackEvent) -> ApiResult<()> {
        let payload = Payload::of(event);
        // FX player tracks, and tracks that send no metadata (jingles), go
        // only to webhooks that ask for them.
        let entry = match event {
            TrackEvent::Started { entry, .. } | TrackEvent::Ended { entry, .. } => entry,
        };
        let fx = entry.track.no_metadata
            || payload
                .track
                .as_ref()
                .is_some_and(|track| track.deck == DeckName::Fx);
        let webhooks: Vec<(u64, String, bool)> = sqlx::query_as(
            "SELECT id, CAST(events AS CHAR), include_fx FROM webhooks WHERE enabled ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut queued = false;
        for (id, events, include_fx) in webhooks {
            let events: Vec<WebhookEvent> = serde_json::from_str(&events).unwrap_or_default();
            if events.contains(&payload.event) && (include_fx || !fx) {
                self.queue(id, payload.clone()).await?;
                queued = true;
            }
        }
        if queued {
            self.wake.notify_one();
        }
        Ok(())
    }

    /// Adds a delivery, due now. The body is written once the delivery has
    /// its ID, which it carries.
    async fn queue(&self, webhook_id: u64, mut payload: Payload) -> ApiResult<u64> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO webhook_deliveries (webhook_id, event, body) VALUES (?, ?, '')",
        )
        .bind(webhook_id)
        .bind(payload.event.as_str())
        .execute(&mut *tx)
        .await?
        .last_insert_id();
        let (content_type, template): (String, Option<String>) =
            sqlx::query_as("SELECT content_type, body_template FROM webhooks WHERE id = ?")
                .bind(webhook_id)
                .fetch_one(&mut *tx)
                .await?;
        payload.delivery_id = id;
        sqlx::query(
            "UPDATE webhook_deliveries SET body = ?, next_attempt_at = NOW(6) WHERE id = ?",
        )
        .bind(body(template.as_deref(), &content_type, &payload))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Sends due deliveries, a few at a time, and keeps the log to its
    /// length.
    pub async fn send_loop(self: Arc<Self>) {
        let mut sending = JoinSet::new();
        let mut pruned_at: Option<tokio::time::Instant> = None;
        loop {
            if pruned_at.is_none_or(|at| at.elapsed() >= PRUNE_INTERVAL) {
                if let Err(error) = self.prune().await {
                    tracing::warn!(%error, "cannot prune the webhook delivery log");
                }
                pruned_at = Some(tokio::time::Instant::now());
            }
            let mut wait = IDLE;
            if sending.len() < MAX_IN_FLIGHT {
                match self.claim(MAX_IN_FLIGHT - sending.len()).await {
                    Ok(ids) => {
                        for id in ids {
                            sending.spawn(Arc::clone(&self).send(id));
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "cannot read the webhook deliveries");
                        wait = Duration::from_secs(5);
                    }
                }
                if let Ok(Some(next)) = self.next_due().await {
                    wait = wait.min(next);
                }
            }
            tokio::select! {
                Some(_) = sending.join_next(), if !sending.is_empty() => {}
                () = self.wake.notified() => {}
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Takes due deliveries: they are due again in a minute, in case the
    /// API stops while sending them.
    async fn claim(&self, limit: usize) -> ApiResult<Vec<u64>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<u64> = sqlx::query_scalar(
            "SELECT id FROM webhook_deliveries
             WHERE status = 'pending' AND next_attempt_at <= NOW(6)
             ORDER BY next_attempt_at, id LIMIT ? FOR UPDATE",
        )
        .bind(u32::try_from(limit).unwrap_or(u32::MAX))
        .fetch_all(&mut *tx)
        .await?;
        for id in &ids {
            sqlx::query(
                "UPDATE webhook_deliveries SET next_attempt_at = NOW(6) + INTERVAL ? MICROSECOND
                 WHERE id = ?",
            )
            .bind(CLAIM.as_micros() as u64)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// How long until the next pending delivery is due.
    async fn next_due(&self) -> ApiResult<Option<Duration>> {
        let micros: Option<i64> = sqlx::query_scalar(
            "SELECT CAST(GREATEST(0, TIMESTAMPDIFF(MICROSECOND, NOW(6), MIN(next_attempt_at)))
                         AS SIGNED)
             FROM webhook_deliveries WHERE status = 'pending'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(micros.map(|micros| Duration::from_micros(micros.unsigned_abs())))
    }

    async fn prune(&self) -> ApiResult<()> {
        let pruned = sqlx::query(
            "DELETE FROM webhook_deliveries
             WHERE created_at < NOW(6) - INTERVAL ? DAY AND status <> 'pending'",
        )
        .bind(KEEP_DAYS)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if pruned > 0 {
            tracing::info!(pruned, "pruned old webhook deliveries");
        }
        Ok(())
    }

    async fn send(self: Arc<Self>, id: u64) {
        if let Err(error) = self.attempt(id).await {
            tracing::error!(%error, id, "cannot send a webhook delivery");
        }
    }

    async fn attempt(&self, id: u64) -> ApiResult<()> {
        let row: Option<DueRow> = sqlx::query_as(
            "SELECT w.name, w.url, w.secret, w.content_type, d.event, d.attempts, w.enabled
             FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id
             WHERE d.id = ? AND d.status = 'pending'",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        // Deleted, or retried by hand meanwhile.
        let Some((name, url, secret, content_type, event, attempts, enabled)) = row else {
            return Ok(());
        };
        if !enabled {
            sqlx::query(
                "UPDATE webhook_deliveries SET status = 'failed', next_attempt_at = NULL,
                        last_error = 'the webhook is disabled'
                 WHERE id = ?",
            )
            .bind(id)
            .execute(&self.pool)
            .await?;
            return Ok(());
        }
        let body: String = sqlx::query_scalar("SELECT body FROM webhook_deliveries WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        let timestamp = unix_ms(SystemTime::now()) / 1_000;
        let result = self
            .client
            .post(&url)
            .header("content-type", &content_type)
            .header("x-heron-event", &event)
            .header("x-heron-delivery", id.to_string())
            .header("x-heron-timestamp", timestamp.to_string())
            .header("x-heron-signature", signature(&secret, timestamp, &body))
            .body(body)
            .send()
            .await;
        let (code, response, error) = match result {
            Ok(mut response) => {
                let code = response.status();
                let mut text = Vec::new();
                while text.len() < RESPONSE_CHARS * 4 {
                    match response.chunk().await {
                        Ok(Some(chunk)) => text.extend_from_slice(&chunk),
                        _ => break,
                    }
                }
                let text = truncate(String::from_utf8_lossy(&text).into_owned(), RESPONSE_CHARS);
                let error = (!code.is_success()).then(|| format!("answered {code}"));
                (Some(code.as_u16()), Some(text), error)
            }
            Err(error) => (None, None, Some(truncate(error_chain(&error), ERROR_CHARS))),
        };
        let attempts = attempts.saturating_add(1);
        let next = error.as_ref().and_then(|_| self.backoff.after(attempts));
        let status = match (&error, next) {
            (None, _) => DeliveryStatus::Delivered,
            (Some(_), Some(_)) => DeliveryStatus::Pending,
            (Some(_), None) => DeliveryStatus::Failed,
        };
        sqlx::query(
            "UPDATE webhook_deliveries
             SET status = ?, attempts = ?, last_attempt_at = NOW(6), last_status_code = ?,
                 last_error = ?, last_response = ?,
                 next_attempt_at = IF(? IS NULL, NULL, NOW(6) + INTERVAL ? MICROSECOND),
                 delivered_at = IF(? = 'delivered', NOW(6), delivered_at)
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(attempts)
        .bind(code)
        .bind(&error)
        .bind(&response)
        .bind(next.map(|next| next.as_micros() as u64))
        .bind(next.map(|next| next.as_micros() as u64))
        .bind(status.as_str())
        .bind(id)
        .execute(&self.pool)
        .await?;
        let result = match status {
            DeliveryStatus::Delivered => "delivered",
            DeliveryStatus::Pending => "retrying",
            DeliveryStatus::Failed => "failed",
        };
        metrics::counter!("heron_api_webhook_deliveries_total", "result" => result).increment(1);
        match (status, &error) {
            (DeliveryStatus::Pending, Some(error)) => {
                tracing::info!(webhook = %name, id, attempts, %error, "webhook delivery failed, will retry");
            }
            (DeliveryStatus::Failed, Some(error)) => {
                tracing::warn!(webhook = %name, id, attempts, %error, "webhook delivery failed, giving up");
            }
            _ => {}
        }
        Ok(())
    }
}

/// A webhook, without its secret.
#[derive(Clone, Debug, Serialize, ToSchema, PartialEq)]
pub struct WebhookView {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
    /// Also send the FX player's tracks (jingles, liners).
    pub include_fx: bool,
    pub content_type: String,
    /// Missing: the default JSON body (`Payload`).
    pub body_template: Option<String>,
    /// The latest delivery.
    pub last_delivery: Option<DeliverySummary>,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, ToSchema, PartialEq)]
pub struct DeliverySummary {
    pub status: DeliveryStatus,
    pub created_at_unix_ms: i64,
    pub last_status_code: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookInput {
    pub name: String,
    /// `http` or `https`. It may be an address of your network.
    pub url: String,
    /// At least one of `track.started`, `track.ended`, `track.skipped`.
    pub events: Vec<WebhookEvent>,
    #[serde(default)]
    pub include_fx: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// `application/json` by default.
    pub content_type: Option<String>,
    /// With `{{placeholders}}`: `event`, `delivery_id`, `at_unix_ms`,
    /// `track_id`, `artist`, `title`, `album`, `duration_ms`, `deck`,
    /// `source`, `played_ms`, `reason`. Missing or empty: the default JSON
    /// body.
    pub body_template: Option<String>,
}

fn yes() -> bool {
    true
}

/// A new webhook, with its secret: the only time it is shown.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CreatedWebhook {
    pub webhook: WebhookView,
    pub secret: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WebhookSecret {
    /// Shown once.
    pub secret: String,
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct DeliveryView {
    pub id: u64,
    pub webhook_id: u64,
    #[schema(value_type = WebhookEvent)]
    pub event: String,
    #[schema(value_type = DeliveryStatus)]
    pub status: String,
    pub attempts: u16,
    /// What was sent.
    pub body: String,
    pub created_at_unix_ms: i64,
    pub last_attempt_at_unix_ms: Option<i64>,
    /// While pending.
    pub next_attempt_at_unix_ms: Option<i64>,
    pub delivered_at_unix_ms: Option<i64>,
    /// The receiver's HTTP status.
    pub last_status_code: Option<u16>,
    /// Why the last attempt failed.
    pub last_error: Option<String>,
    /// The start of the receiver's last answer.
    pub last_response: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeliveriesQuery {
    /// Deliveries older than this one (its ID), for the next page.
    pub before: Option<u64>,
    pub status: Option<DeliveryStatus>,
    /// 1 to 200, 50 by default.
    pub limit: Option<u32>,
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_webhooks, create_webhook))
        .routes(routes!(get_webhook, update_webhook, delete_webhook))
        .routes(routes!(rotate_secret))
        .routes(routes!(test_webhook))
        .routes(routes!(list_deliveries))
        .routes(routes!(retry_delivery))
}

struct Checked {
    name: String,
    url: String,
    events: String,
    content_type: String,
    body_template: Option<String>,
}

fn check(input: WebhookInput) -> ApiResult<Checked> {
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "the name is 1 to 255 characters".to_owned(),
        ));
    }
    let url = input.url.trim().to_owned();
    let parsed = reqwest::Url::parse(&url)
        .map_err(|error| ApiError::Invalid(format!("invalid URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(ApiError::Invalid(
            "the URL must be http or https, with a host".to_owned(),
        ));
    }
    if url.len() > 2_048 {
        return Err(ApiError::Invalid(
            "the URL is longer than 2048 characters".to_owned(),
        ));
    }
    let mut events = input.events;
    events.dedup();
    if events.is_empty() || events.contains(&WebhookEvent::Ping) {
        return Err(ApiError::Invalid(
            "events: at least one of track.started, track.ended and track.skipped".to_owned(),
        ));
    }
    let content_type = input
        .content_type
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "application/json".to_owned());
    if content_type.len() > 255 || HeaderValue::from_str(&content_type).is_err() {
        return Err(ApiError::Invalid("invalid content type".to_owned()));
    }
    let body_template = input.body_template.filter(|text| !text.trim().is_empty());
    if let Some(template) = &body_template {
        check_template(template).map_err(ApiError::Invalid)?;
    }
    Ok(Checked {
        name,
        url,
        events: serde_json::to_string(&events).unwrap_or_default(),
        content_type,
        body_template,
    })
}

fn duplicate_or(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("a webhook already has this name".to_owned())
    } else {
        error.into()
    }
}

type WebhookRow = (
    u64,
    String,
    String,
    bool,
    String,
    bool,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<i64>,
    Option<u16>,
);

async fn load_webhooks(pool: &MySqlPool, id: Option<u64>) -> ApiResult<Vec<WebhookView>> {
    let rows: Vec<WebhookRow> = sqlx::query_as(
        "SELECT w.id, w.name, w.url, w.enabled, CAST(w.events AS CHAR), w.include_fx,
                w.content_type, w.body_template,
                CAST(UNIX_TIMESTAMP(w.created_at) * 1000 AS SIGNED),
                CAST(d.status AS CHAR), CAST(UNIX_TIMESTAMP(d.created_at) * 1000 AS SIGNED),
                d.last_status_code
         FROM webhooks w
         LEFT JOIN webhook_deliveries d ON d.id =
             (SELECT MAX(id) FROM webhook_deliveries WHERE webhook_id = w.id)
         WHERE ? IS NULL OR w.id = ?
         ORDER BY w.name",
    )
    .bind(id)
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                url,
                enabled,
                events,
                include_fx,
                content_type,
                body_template,
                created_at_unix_ms,
                status,
                delivery_at,
                last_status_code,
            )| WebhookView {
                id,
                name,
                url,
                enabled,
                events: serde_json::from_str(&events).unwrap_or_default(),
                include_fx,
                content_type,
                body_template,
                last_delivery: status.zip(delivery_at).map(|(status, at)| DeliverySummary {
                    status: DeliveryStatus::parse(&status),
                    created_at_unix_ms: at,
                    last_status_code,
                }),
                created_at_unix_ms,
            },
        )
        .collect())
}

async fn load_webhook(pool: &MySqlPool, id: u64) -> ApiResult<WebhookView> {
    load_webhooks(pool, Some(id))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("webhook"))
}

/// Webhooks, by name. Admins only.
#[utoipa::path(get, path = "/webhooks", tag = "webhooks",
    responses((status = 200, body = Vec<WebhookView>), (status = "4XX", body = ErrorBody)))]
async fn list_webhooks(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<WebhookView>>> {
    user.require(Role::Admin)?;
    Ok(Json(load_webhooks(&state.pool, None).await?))
}

#[utoipa::path(get, path = "/webhooks/{id}", tag = "webhooks",
    params(("id" = u64, Path)),
    responses((status = 200, body = WebhookView), (status = "4XX", body = ErrorBody)))]
async fn get_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<WebhookView>> {
    user.require(Role::Admin)?;
    Ok(Json(load_webhook(&state.pool, id).await?))
}

/// Adds a webhook. The answer holds its secret, shown only this once.
/// Admins only.
#[utoipa::path(post, path = "/webhooks", tag = "webhooks",
    request_body = WebhookInput,
    responses((status = 201, body = CreatedWebhook), (status = "4XX", body = ErrorBody)))]
async fn create_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<WebhookInput>,
) -> ApiResult<(StatusCode, Json<CreatedWebhook>)> {
    user.require(Role::Admin)?;
    let enabled = input.enabled;
    let include_fx = input.include_fx;
    let checked = check(input)?;
    let secret = new_secret()?;
    let id = sqlx::query(
        "INSERT INTO webhooks (name, url, enabled, events, include_fx, secret, content_type,
                               body_template)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&checked.name)
    .bind(&checked.url)
    .bind(enabled)
    .bind(&checked.events)
    .bind(include_fx)
    .bind(&secret)
    .bind(&checked.content_type)
    .bind(&checked.body_template)
    .execute(&state.pool)
    .await
    .map_err(duplicate_or)?
    .last_insert_id();
    tracing::info!(user = %user.username, id, name = %checked.name, "webhook created");
    let webhook = load_webhook(&state.pool, id).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedWebhook { webhook, secret }),
    ))
}

/// Replaces a webhook; its secret is kept. Admins only.
#[utoipa::path(put, path = "/webhooks/{id}", tag = "webhooks",
    params(("id" = u64, Path)), request_body = WebhookInput,
    responses((status = 200, body = WebhookView), (status = "4XX", body = ErrorBody)))]
async fn update_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<WebhookInput>,
) -> ApiResult<Json<WebhookView>> {
    user.require(Role::Admin)?;
    let enabled = input.enabled;
    let include_fx = input.include_fx;
    let checked = check(input)?;
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM webhooks WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("webhook"));
    }
    sqlx::query(
        "UPDATE webhooks SET name = ?, url = ?, enabled = ?, events = ?, include_fx = ?,
                content_type = ?, body_template = ?
         WHERE id = ?",
    )
    .bind(&checked.name)
    .bind(&checked.url)
    .bind(enabled)
    .bind(&checked.events)
    .bind(include_fx)
    .bind(&checked.content_type)
    .bind(&checked.body_template)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(duplicate_or)?;
    tracing::info!(user = %user.username, id, name = %checked.name, "webhook changed");
    Ok(Json(load_webhook(&state.pool, id).await?))
}

/// Deletes a webhook and its deliveries. Admins only.
#[utoipa::path(delete, path = "/webhooks/{id}", tag = "webhooks",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let deleted = sqlx::query("DELETE FROM webhooks WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("webhook"));
    }
    tracing::info!(user = %user.username, id, "webhook deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// Gives a webhook a new secret, shown only this once; deliveries not sent
/// yet are signed with it. Admins only.
#[utoipa::path(post, path = "/webhooks/{id}/secret", tag = "webhooks",
    params(("id" = u64, Path)),
    responses((status = 200, body = WebhookSecret), (status = "4XX", body = ErrorBody)))]
async fn rotate_secret(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<WebhookSecret>> {
    user.require(Role::Admin)?;
    let secret = new_secret()?;
    let updated = sqlx::query("UPDATE webhooks SET secret = ? WHERE id = ?")
        .bind(&secret)
        .bind(id)
        .execute(&state.pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found("webhook"));
    }
    tracing::info!(user = %user.username, id, "webhook secret changed");
    Ok(Json(WebhookSecret { secret }))
}

const DELIVERY_COLUMNS: &str = "SELECT id, webhook_id, event, CAST(status AS CHAR) AS status,
        attempts, body,
        CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED) AS created_at_unix_ms,
        CAST(UNIX_TIMESTAMP(last_attempt_at) * 1000 AS SIGNED) AS last_attempt_at_unix_ms,
        CAST(UNIX_TIMESTAMP(next_attempt_at) * 1000 AS SIGNED) AS next_attempt_at_unix_ms,
        CAST(UNIX_TIMESTAMP(delivered_at) * 1000 AS SIGNED) AS delivered_at_unix_ms,
        last_status_code, last_error, last_response
 FROM webhook_deliveries";

async fn load_delivery(pool: &MySqlPool, id: u64) -> ApiResult<DeliveryView> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{DELIVERY_COLUMNS} WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("delivery"))
}

/// Sends a `ping` delivery now, whether the webhook is enabled or not.
/// Admins only.
#[utoipa::path(post, path = "/webhooks/{id}/test", tag = "webhooks",
    params(("id" = u64, Path)),
    responses((status = 202, body = DeliveryView), (status = "4XX", body = ErrorBody)))]
async fn test_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<(StatusCode, Json<DeliveryView>)> {
    user.require(Role::Admin)?;
    load_webhook(&state.pool, id).await?;
    let delivery = state.webhooks.queue(id, Payload::ping()).await?;
    state.webhooks.wake.notify_one();
    Ok((
        StatusCode::ACCEPTED,
        Json(load_delivery(&state.pool, delivery).await?),
    ))
}

/// A webhook's deliveries, most recent first. Admins only.
#[utoipa::path(get, path = "/webhooks/{id}/deliveries", tag = "webhooks",
    params(("id" = u64, Path), DeliveriesQuery),
    responses((status = 200, body = Vec<DeliveryView>), (status = "4XX", body = ErrorBody)))]
async fn list_deliveries(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<DeliveriesQuery>,
) -> ApiResult<Json<Vec<DeliveryView>>> {
    user.require(Role::Admin)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 200".to_owned()));
    }
    load_webhook(&state.pool, id).await?;
    let mut sql = sqlx::QueryBuilder::new(DELIVERY_COLUMNS);
    sql.push(" WHERE webhook_id = ").push_bind(id);
    if let Some(before) = query.before {
        sql.push(" AND id < ").push_bind(before);
    }
    if let Some(status) = query.status {
        sql.push(" AND status = ").push_bind(status.as_str());
    }
    sql.push(" ORDER BY id DESC LIMIT ").push_bind(limit);
    Ok(Json(sql.build_query_as().fetch_all(&state.pool).await?))
}

/// Sends a delivered or failed delivery again, with every attempt again.
/// Admins only.
#[utoipa::path(post, path = "/webhook-deliveries/{id}/retry", tag = "webhooks",
    params(("id" = u64, Path)),
    responses((status = 202, body = DeliveryView), (status = "4XX", body = ErrorBody)))]
async fn retry_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<(StatusCode, Json<DeliveryView>)> {
    user.require(Role::Admin)?;
    let delivery = load_delivery(&state.pool, id).await?;
    if delivery.status == DeliveryStatus::Pending.as_str() {
        return Err(ApiError::Conflict(
            "the delivery is still being sent".to_owned(),
        ));
    }
    sqlx::query(
        "UPDATE webhook_deliveries SET status = 'pending', attempts = 0,
                next_attempt_at = NOW(6), delivered_at = NULL
         WHERE id = ? AND status <> 'pending'",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    state.webhooks.wake.notify_one();
    tracing::info!(user = %user.username, id, "webhook delivery retried");
    Ok((
        StatusCode::ACCEPTED,
        Json(load_delivery(&state.pool, id).await?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{QueueEntry, QueuedTrack};

    fn entry(title: &str) -> QueueEntry {
        QueueEntry {
            entry_id: 7,
            track: QueuedTrack {
                track_id: Some(3),
                path: "a/b.flac".to_owned(),
                title: Some(title.to_owned()),
                artist: Some("Simon & Garfunkel".to_owned()),
                album: None,
                duration_ms: 180_000,
                cue_in_ms: 0,
                cue_out_ms: 180_000,
                source: PlaySource::Rotation,
                no_metadata: false,
            },
        }
    }

    fn ended(reason: EndReason) -> TrackEvent {
        TrackEvent::Ended {
            deck: DeckName::A,
            entry: entry("Song"),
            at_unix_ms: 1_000,
            played_ms: 500,
            reason,
        }
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hmac_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            signature("Jefe", 12, "{}"),
            format!("sha256={}", hmac_hex(b"Jefe", b"12.{}"))
        );
    }

    #[test]
    fn skips_end_as_skipped_and_other_ends_as_ended() {
        let started = TrackEvent::Started {
            deck: DeckName::B,
            entry: entry("Song"),
            at_unix_ms: 1,
        };
        assert_eq!(WebhookEvent::of(&started), WebhookEvent::TrackStarted);
        assert_eq!(
            WebhookEvent::of(&ended(EndReason::Skipped)),
            WebhookEvent::TrackSkipped
        );
        for reason in [EndReason::Finished, EndReason::Stopped, EndReason::Replaced] {
            assert_eq!(WebhookEvent::of(&ended(reason)), WebhookEvent::TrackEnded);
        }
    }

    #[test]
    fn the_default_body_is_the_payload() {
        let mut payload = Payload::of(&ended(EndReason::Skipped));
        payload.delivery_id = 9;
        let value: serde_json::Value =
            serde_json::from_str(&body(None, "application/json", &payload)).unwrap();
        assert_eq!(value["event"], "track.skipped");
        assert_eq!(value["delivery_id"], 9);
        assert_eq!(value["track"]["artist"], "Simon & Garfunkel");
        assert_eq!(value["track"]["deck"], "a");
        assert_eq!(value["track"]["source"], "rotation");
        assert_eq!(value["played_ms"], 500);
        assert_eq!(value["reason"], "skipped");
    }

    #[test]
    fn templates_escape_json_and_leave_missing_values_empty() {
        let mut started = Payload::of(&TrackEvent::Started {
            deck: DeckName::A,
            entry: entry("Say \"hi\"\n"),
            at_unix_ms: 5,
        });
        started.delivery_id = 2;
        let template = r#"{"content": "{{ artist }} – {{title}}", "album": "{{album}}", "played": "{{played_ms}}", "n": {{delivery_id}}}"#;
        let rendered = render(template, "application/json; charset=utf-8", &started);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["content"], "Simon & Garfunkel – Say \"hi\"\n");
        assert_eq!(value["album"], "");
        assert_eq!(value["played"], "");
        assert_eq!(value["n"], 2);
        assert_eq!(
            render("{{title}} ({{source}}, {{deck}})", "text/plain", &started),
            "Say \"hi\"\n (rotation, a)"
        );
        assert_eq!(
            render("{{event}} {{ unclosed", "text/plain", &started),
            "track.started {{ unclosed"
        );
    }

    #[test]
    fn every_placeholder_is_filled() {
        let template = "{{event}}|{{artist}}|{{title}}|{{album}}|{{duration_ms}}|{{played_ms}}|\
                        {{reason}}|{{deck}}|{{source}}|{{track_id}}|{{at_unix_ms}}|{{delivery_id}}";
        let mut ended = Payload::of(&ended(EndReason::Finished));
        ended.delivery_id = 4;
        assert_eq!(
            render(template, "text/plain", &ended),
            "track.ended|Simon & Garfunkel|Song||180000|500|finished|a|rotation|3|1000|4"
        );
        let mut ping = Payload::ping();
        ping.at_unix_ms = 7;
        assert_eq!(render(template, "text/plain", &ping), "ping||||||||||7|0");
    }

    #[test]
    fn templates_with_unknown_placeholders_are_refused() {
        assert!(check_template("{{artist}} {{ title }} {{reason}}").is_ok());
        let error = check_template("{{artist}} {{song}}").unwrap_err();
        assert!(error.contains("song"), "{error}");
        assert!(check_template(&"x".repeat(MAX_TEMPLATE_BYTES + 1)).is_err());
    }

    #[test]
    fn backoff_gives_up_after_the_last_wait() {
        let backoff = Backoff::default();
        assert_eq!(backoff.after(0), None);
        assert_eq!(backoff.after(1), Some(Duration::from_secs(10)));
        assert_eq!(backoff.after(7), Some(Duration::from_secs(10_800)));
        assert_eq!(backoff.after(8), None);
    }

    #[test]
    fn inputs_are_checked() {
        let input =
            |url: &str, events: Vec<WebhookEvent>, content_type: Option<&str>| WebhookInput {
                name: " Website ".to_owned(),
                url: url.to_owned(),
                events,
                include_fx: false,
                enabled: true,
                content_type: content_type.map(str::to_owned),
                body_template: Some("  ".to_owned()),
            };
        let checked = check(input(
            "https://example.org/hook",
            vec![WebhookEvent::TrackStarted],
            None,
        ))
        .unwrap();
        assert_eq!(checked.name, "Website");
        assert_eq!(checked.events, r#"["track.started"]"#);
        assert_eq!(checked.content_type, "application/json");
        assert_eq!(checked.body_template, None);
        for (url, events, content_type) in [
            ("ftp://example.org", vec![WebhookEvent::TrackStarted], None),
            ("not a url", vec![WebhookEvent::TrackStarted], None),
            ("http://example.org", vec![], None),
            ("http://example.org", vec![WebhookEvent::Ping], None),
            (
                "http://example.org",
                vec![WebhookEvent::TrackEnded],
                Some("text/plain\r\nX-Evil: 1"),
            ),
        ] {
            assert!(check(input(url, events, content_type)).is_err(), "{url}");
        }
        let long_url = format!("https://example.org/{}", "a".repeat(2_048));
        assert!(check(input(&long_url, vec![WebhookEvent::TrackStarted], None)).is_err());
        for name in ["  ", &"n".repeat(256)] {
            let mut unnamed = input(
                "https://example.org",
                vec![WebhookEvent::TrackStarted],
                None,
            );
            unnamed.name = name.to_owned();
            assert!(check(unnamed).is_err(), "{name}");
        }
        let mut templated = input(
            "https://example.org",
            vec![WebhookEvent::TrackStarted],
            None,
        );
        templated.body_template = Some("{{song}}".to_owned());
        assert!(check(templated).is_err());
    }
}

#[cfg(test)]
mod delivery_tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::http::HeaderMap;
    use heron_service::db::ScratchDatabase;

    use super::*;
    use crate::engine::{QueueEntry, QueuedTrack};

    /// Answers 500 to the first `failures` requests, then 200, and keeps
    /// what it got.
    struct Receiver {
        failures: usize,
        requests: AtomicUsize,
        got: Mutex<Vec<(HeaderMap, String)>>,
    }

    async fn receiver(failures: usize) -> (Arc<Receiver>, String) {
        let receiver = Arc::new(Receiver {
            failures,
            requests: AtomicUsize::new(0),
            got: Mutex::new(Vec::new()),
        });
        let shared = Arc::clone(&receiver);
        let app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let receiver = Arc::clone(&shared);
                async move {
                    receiver.got.lock().unwrap().push((headers, body));
                    if receiver.requests.fetch_add(1, Ordering::SeqCst) < receiver.failures {
                        (StatusCode::INTERNAL_SERVER_ERROR, "down for maintenance")
                    } else {
                        (StatusCode::OK, "thanks")
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (receiver, url)
    }

    async fn add_webhook(pool: &MySqlPool, name: &str, url: &str) -> u64 {
        sqlx::query(
            "INSERT INTO webhooks (name, url, events, secret)
             VALUES (?, ?, '[\"track.started\"]', 'whsec_test')",
        )
        .bind(name)
        .bind(url)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id()
    }

    fn started(deck: DeckName) -> TrackEvent {
        TrackEvent::Started {
            deck,
            entry: QueueEntry {
                entry_id: 1,
                track: QueuedTrack {
                    track_id: None,
                    path: "song.flac".to_owned(),
                    title: Some("Song".to_owned()),
                    artist: Some("Artist".to_owned()),
                    album: None,
                    duration_ms: 1_000,
                    cue_in_ms: 0,
                    cue_out_ms: 1_000,
                    source: PlaySource::Rotation,
                    no_metadata: false,
                },
            },
            at_unix_ms: unix_ms(SystemTime::now()),
        }
    }

    async fn delivery(pool: &MySqlPool, webhook_id: u64) -> Vec<(String, u16, Option<u16>)> {
        sqlx::query_as(
            "SELECT CAST(status AS CHAR), attempts, last_status_code FROM webhook_deliveries
             WHERE webhook_id = ? ORDER BY id",
        )
        .bind(webhook_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn settled(pool: &MySqlPool, webhook_id: u64) -> Vec<(String, u16, Option<u16>)> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let rows = delivery(pool, webhook_id).await;
            if !rows.is_empty() && rows.iter().all(|(status, ..)| status != "pending") {
                return rows;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "still pending: {rows:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn failed_deliveries_are_retried_until_they_give_up() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let webhooks =
            Webhooks::with_backoff(db.pool.clone(), Backoff(vec![Duration::from_millis(50); 3]))
                .unwrap();
        let sender = tokio::spawn(Arc::clone(&webhooks).send_loop());
        let (flaky, flaky_url) = receiver(2).await;
        let (down, down_url) = receiver(usize::MAX).await;
        let flaky_id = add_webhook(&db.pool, "flaky", &flaky_url).await;
        let down_id = add_webhook(&db.pool, "down", &down_url).await;

        // FX tracks go only to webhooks that ask for them.
        webhooks.queue_event(&started(DeckName::Fx)).await.unwrap();
        webhooks.queue_event(&started(DeckName::A)).await.unwrap();

        assert_eq!(
            settled(&db.pool, flaky_id).await,
            [("delivered".to_owned(), 3, Some(200))]
        );
        assert_eq!(
            settled(&db.pool, down_id).await,
            [("failed".to_owned(), 4, Some(500))]
        );
        assert_eq!(flaky.requests.load(Ordering::SeqCst), 3);
        assert_eq!(down.requests.load(Ordering::SeqCst), 4);

        // Every attempt carries the same signed body.
        let got = flaky.got.lock().unwrap().clone();
        for (headers, body) in &got {
            let timestamp: u64 = headers["x-heron-timestamp"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                headers["x-heron-signature"].to_str().unwrap(),
                signature("whsec_test", timestamp, body)
            );
            assert_eq!(headers["x-heron-event"], "track.started");
            assert_eq!(body, &got[0].1);
        }
        let payload: serde_json::Value = serde_json::from_str(&got[0].1).unwrap();
        assert_eq!(payload["track"]["deck"], "a");
        assert_eq!(
            headers_delivery(&got[0].0),
            payload["delivery_id"].as_u64().unwrap()
        );

        let (_, response, error): (u16, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT attempts, last_response, last_error FROM webhook_deliveries WHERE webhook_id = ?",
        )
        .bind(down_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(response.as_deref(), Some("down for maintenance"));
        assert_eq!(error.as_deref(), Some("answered 500 Internal Server Error"));

        sender.abort();
        db.drop().await;
    }

    fn headers_delivery(headers: &HeaderMap) -> u64 {
        headers["x-heron-delivery"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn disabled_webhooks_send_nothing() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let webhooks = Webhooks::with_backoff(db.pool.clone(), Backoff(vec![])).unwrap();
        let (receiver, url) = receiver(0).await;
        let id = add_webhook(&db.pool, "off", &url).await;
        // A delivery queued before the webhook was turned off.
        webhooks.queue(id, Payload::ping()).await.unwrap();
        sqlx::query("UPDATE webhooks SET enabled = FALSE WHERE id = ?")
            .bind(id)
            .execute(&db.pool)
            .await
            .unwrap();
        webhooks.queue_event(&started(DeckName::A)).await.unwrap();
        let sender = tokio::spawn(Arc::clone(&webhooks).send_loop());
        assert_eq!(
            settled(&db.pool, id).await,
            [("failed".to_owned(), 0, None)]
        );
        assert!(receiver.got.lock().unwrap().is_empty());
        sender.abort();
        db.drop().await;
    }

    #[tokio::test]
    async fn unreachable_receivers_are_retried() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let webhooks =
            Webhooks::with_backoff(db.pool.clone(), Backoff(vec![Duration::from_millis(50)]))
                .unwrap();
        let sender = tokio::spawn(Arc::clone(&webhooks).send_loop());
        // Nothing listens there.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        drop(listener);
        let id = add_webhook(&db.pool, "gone", &url).await;
        webhooks.queue_event(&started(DeckName::B)).await.unwrap();
        assert_eq!(
            settled(&db.pool, id).await,
            [("failed".to_owned(), 2, None)]
        );
        let error: Option<String> =
            sqlx::query_scalar("SELECT last_error FROM webhook_deliveries WHERE webhook_id = ?")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(error.is_some_and(|error| error.contains("connect")));
        sender.abort();
        db.drop().await;
    }
}
