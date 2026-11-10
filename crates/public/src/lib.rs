// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-public`: the public gateway of the shows
//! (`docs/listener-interaction.md`). The only part of Heron listeners
//! reach: it serves the shows' widgets and state from its cache, follows
//! the API with one request a second, and checks listeners' writes before
//! they go on.

pub mod actions;
pub mod app;
pub mod limits;
pub mod proof;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderName};
use heron_core::Module;
use heron_core::shows::ShowPublic;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::actions::Action;
use crate::limits::{Counters, PerMinute, Streams};
use crate::proof::Proofs;

pub const SERVICE: &str = "heron-public";

/// Requests to the API wait this long at most.
const API_TIMEOUT: Duration = Duration::from_secs(5);
/// Counts and challenges that expired are forgotten this often.
const PRUNE_EVERY: Duration = Duration::from_secs(60);

/// The gateway's settings (`docs/listener-interaction.md#settings`).
#[derive(Clone, Debug)]
pub struct Config {
    /// `http://heron-api:8080`.
    pub api_url: String,
    /// The gateway token; none, and the gateway serves nothing of the shows.
    pub token: Option<String>,
    /// The websites that may send writes; empty: any.
    pub allowed_origins: Vec<String>,
    /// The header a reverse proxy puts the listener's address in.
    pub client_ip_header: Option<HeaderName>,
    pub reads_per_minute: u32,
    pub writes_per_minute: u32,
    pub writes_per_second: u32,
    pub streams_per_address: u32,
    pub max_streams: usize,
    pub proof_bits: u8,
    /// How often the API's state is read.
    pub poll: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_url: "http://heron-api:8080".to_owned(),
            token: None,
            allowed_origins: Vec::new(),
            client_ip_header: None,
            reads_per_minute: 300,
            writes_per_minute: 20,
            writes_per_second: 50,
            streams_per_address: 10,
            max_streams: 10_000,
            proof_bits: 16,
            poll: Duration::from_secs(1),
        }
    }
}

/// A station as the API gives it (`GET /api/v1/gateway/state`).
#[derive(Clone, Debug, Deserialize)]
pub struct ApiStation {
    pub code: String,
    pub name: String,
    pub timezone: String,
    /// As `GET /public/now-playing` gives it, passed on as it is.
    pub now_playing: Value,
    pub shows: Vec<ShowPublic>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiState {
    pub stations: Vec<ApiStation>,
}

/// A station the gateway serves, and the widgets following it.
struct Station {
    data: ApiStation,
    events: watch::Sender<Arc<String>>,
    /// What was last sent, without what moves on its own (positions).
    sent: String,
}

/// Whether the API answers.
struct ApiHealth {
    started: Instant,
    last_ok: Option<Instant>,
}

/// Writes waiting for the next batch, by path (`main/match/vote`).
type Batches = HashMap<String, Batch>;

/// The writes of one path until they are sent.
enum Batch {
    /// Added up by fields: the fields and their count.
    Counts(HashMap<String, (Map<String, Value>, u64)>),
    /// The last write of each device, by its hash.
    Latest(HashMap<String, Map<String, Value>>),
}

impl Batch {
    /// The body sent to the API.
    fn body(self) -> Value {
        match self {
            Self::Counts(counts) => {
                let counts: Vec<Value> = counts
                    .into_values()
                    .map(|(fields, count)| json!({"fields": fields, "count": count}))
                    .collect();
                json!({ "counts": counts })
            }
            Self::Latest(writes) => {
                let writes: Vec<Value> = writes
                    .into_iter()
                    .map(|(device, fields)| json!({"device": device, "fields": fields}))
                    .collect();
                json!({ "writes": writes })
            }
        }
    }
}

pub struct Gateway {
    pub config: Config,
    http: reqwest::Client,
    /// The writes the shows take.
    pub actions: Vec<Action>,
    pub proofs: Proofs,
    reads: PerMinute,
    writes: PerMinute,
    api_writes: Counters<()>,
    devices: Counters<String>,
    pub streams: Streams,
    stations: RwLock<HashMap<String, Station>>,
    health: Mutex<ApiHealth>,
    cache: app::ReadCache,
    batches: Mutex<Batches>,
}

impl Gateway {
    pub fn new(config: Config, actions: Vec<Action>) -> anyhow::Result<Arc<Self>> {
        let proofs = Proofs::new(config.proof_bits)?;
        Ok(Self::with_proofs(config, actions, proofs)?)
    }

    pub fn with_proofs(
        config: Config,
        actions: Vec<Action>,
        proofs: Proofs,
    ) -> reqwest::Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .timeout(API_TIMEOUT)
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Arc::new(Self {
            reads: PerMinute::new(config.reads_per_minute),
            writes: PerMinute::new(config.writes_per_minute),
            streams: Streams::new(config.streams_per_address, config.max_streams),
            config,
            http,
            actions,
            proofs,
            api_writes: Counters::default(),
            devices: Counters::default(),
            stations: RwLock::default(),
            health: Mutex::new(ApiHealth {
                started: Instant::now(),
                last_ok: None,
            }),
            cache: app::ReadCache::default(),
            batches: Mutex::default(),
        }))
    }

    fn api(&self, path: &str) -> String {
        format!("{}{path}", self.config.api_url.trim_end_matches('/'))
    }

    /// The listener's address: the peer's, or the one the reverse proxy
    /// gives (the last of `X-Forwarded-For`, which the proxy added).
    pub fn client(&self, headers: &HeaderMap, peer: IpAddr) -> IpAddr {
        self.config
            .client_ip_header
            .as_ref()
            .and_then(|name| headers.get(name))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|last| last.trim().parse().ok())
            .unwrap_or(peer)
    }

    /// Whether a website may send writes.
    pub fn origin_allowed(&self, origin: Option<&str>) -> bool {
        let allowed = &self.config.allowed_origins;
        let Some(origin) = origin else {
            // Not a browser on another site.
            return true;
        };
        let normal = |text: &str| text.trim().trim_end_matches('/').to_ascii_lowercase();
        allowed.is_empty() || allowed.iter().any(|one| normal(one) == normal(origin))
    }

    /// Follows the API: its state once per `poll`, the batches sent, the
    /// limits pruned. Never returns.
    pub async fn run(self: Arc<Self>) {
        let mut ticks = tokio::time::interval(self.config.poll);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pruned = Instant::now();
        loop {
            ticks.tick().await;
            let fetched = self.fetch_state().await;
            match &fetched {
                Ok(_) => {
                    self.health
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .last_ok = Some(Instant::now());
                    metrics::gauge!("heron_public_api_up").set(1.0);
                }
                Err(error) => {
                    tracing::debug!(%error, "cannot read the API's state");
                    metrics::gauge!("heron_public_api_up").set(0.0);
                }
            }
            metrics::gauge!("heron_public_state_age_seconds").set(self.state_age().as_secs_f64());
            self.apply(fetched.ok(), Timestamp::now());
            self.flush().await;
            if pruned.elapsed() >= PRUNE_EVERY {
                let now = Instant::now();
                self.reads.prune(now);
                self.writes.prune(now);
                self.devices.prune(now);
                self.api_writes.prune(now);
                self.proofs.prune(Timestamp::now().as_millisecond());
                self.cache.prune();
                pruned = now;
            }
        }
    }

    async fn fetch_state(&self) -> anyhow::Result<ApiState> {
        let token = self
            .config
            .token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("HERON_PUBLIC_TOKEN is not set"))?;
        let response = self
            .http
            .get(self.api("/api/v1/gateway/state"))
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }

    /// Since the API's state was last read; since the start when never.
    pub fn state_age(&self) -> Duration {
        let health = self.health.lock().unwrap_or_else(PoisonError::into_inner);
        health.last_ok.unwrap_or(health.started).elapsed()
    }

    /// Takes the API's state (none: it did not answer, the last one stays)
    /// and tells the widgets what changed, the shows opening and closing
    /// on time either way.
    pub fn apply(&self, fetched: Option<ApiState>, now: Timestamp) {
        let mut stations = self
            .stations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(state) = fetched {
            stations.retain(|code, _| state.stations.iter().any(|station| &station.code == code));
            for data in state.stations {
                match stations.get_mut(&data.code) {
                    Some(station) => station.data = data,
                    None => {
                        let (events, _) = watch::channel(Arc::new(String::new()));
                        stations.insert(
                            data.code.clone(),
                            Station {
                                data,
                                events,
                                sent: String::new(),
                            },
                        );
                    }
                }
            }
        }
        for station in stations.values_mut() {
            let view = station_view(&station.data, now);
            let key = change_key(&view);
            if key != station.sent {
                station.sent = key;
                station.events.send_replace(Arc::new(view.to_string()));
            }
        }
    }

    /// A station's state now, as the event stream sends it.
    pub fn station(&self, code: &str, now: Timestamp) -> Option<Value> {
        let stations = self.stations.read().unwrap_or_else(PoisonError::into_inner);
        stations
            .get(code)
            .map(|station| station_view(&station.data, now))
    }

    /// A show of a station that is on, as the widgets see it now.
    /// A station's show by its key (`match`, `top-4`); `top30` is also the
    /// first top.
    pub fn show(&self, code: &str, key: &str, now: Timestamp) -> Option<Value> {
        let stations = self.stations.read().unwrap_or_else(PoisonError::into_inner);
        let station = stations.get(code)?;
        let shows = &station.data.shows;
        shows
            .iter()
            .find(|show| show.key() == key)
            .or_else(|| {
                (key == Module::Top30.key())
                    .then(|| shows.iter().find(|show| show.module == Module::Top30))
                    .flatten()
            })
            .map(|show| show_view(show, now))
    }

    pub fn has_station(&self, code: &str) -> bool {
        self.stations
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(code)
    }

    /// The station's events, for a widget.
    fn subscribe(&self, code: &str) -> Option<watch::Receiver<Arc<String>>> {
        let stations = self.stations.read().unwrap_or_else(PoisonError::into_inner);
        stations.get(code).map(|station| station.events.subscribe())
    }

    /// Adds a write to its batch.
    fn add_to_batch(&self, path: String, fields: Map<String, Value>) {
        let key = Value::Object(fields.clone()).to_string();
        let mut batches = self.batches.lock().unwrap_or_else(PoisonError::into_inner);
        let batch = batches
            .entry(path)
            .or_insert_with(|| Batch::Counts(HashMap::new()));
        if let Batch::Counts(counts) = batch {
            counts.entry(key).or_insert((fields, 0)).1 += 1;
        }
    }

    /// Keeps a device's write for the next batch, in place of its earlier
    /// one.
    fn keep_latest(&self, path: String, device: &str, fields: Map<String, Value>) {
        let mut batches = self.batches.lock().unwrap_or_else(PoisonError::into_inner);
        let batch = batches
            .entry(path)
            .or_insert_with(|| Batch::Latest(HashMap::new()));
        if let Batch::Latest(writes) = batch {
            writes.insert(device.to_owned(), fields);
        }
    }

    /// Sends the writes added up since the last time, one request per path.
    pub async fn flush(&self) {
        let batches =
            std::mem::take(&mut *self.batches.lock().unwrap_or_else(PoisonError::into_inner));
        let Some(token) = self.config.token.as_deref() else {
            return;
        };
        for (path, batch) in batches {
            let sent = self
                .http
                .post(self.api(&format!("/api/v1/gateway/stations/{path}")))
                .bearer_auth(token)
                .json(&batch.body())
                .send()
                .await
                .and_then(reqwest::Response::error_for_status);
            match sent {
                Ok(_) => metrics::counter!("heron_public_writes_total", "delivery" => "batch")
                    .increment(1),
                Err(error) => tracing::warn!(%path, %error, "a batch of writes was not taken"),
            }
        }
    }
}

/// A show with whether it is open now.
fn show_view(show: &ShowPublic, now: Timestamp) -> Value {
    let status = show.status(now);
    let mut view = serde_json::to_value(show).unwrap_or(Value::Null);
    if let Value::Object(fields) = &mut view {
        fields.insert("open".to_owned(), status.open.into());
        fields.insert(
            "closes_at_unix_ms".to_owned(),
            status.closes_at_unix_ms.into(),
        );
        fields.insert(
            "next_opening_unix_ms".to_owned(),
            status.next_opening_unix_ms.into(),
        );
    }
    view
}

fn station_view(station: &ApiStation, now: Timestamp) -> Value {
    json!({
        "code": station.code,
        "name": station.name,
        "timezone": station.timezone,
        "now_playing": station.now_playing,
        "shows": station.shows.iter().map(|show| show_view(show, now)).collect::<Vec<_>>(),
        "server_time_unix_ms": now.as_millisecond(),
    })
}

/// What tells a change: the view without the clock and the positions,
/// which move on their own.
fn change_key(view: &Value) -> String {
    let mut view = view.clone();
    if let Value::Object(fields) = &mut view {
        fields.remove("server_time_unix_ms");
    }
    if let Some(Value::Object(now_playing)) = view.get_mut("now_playing") {
        now_playing.remove("server_time_unix_ms");
        if let Some(Value::Object(now)) = now_playing.get_mut("now") {
            now.remove("position_ms");
        }
    }
    view.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn state(hours: Value, position_ms: u64) -> ApiState {
        serde_json::from_value(json!({"stations": [{
            "code": "main", "name": "Main", "timezone": "UTC",
            "now_playing": {"now": {"artist": "A", "title": "T", "position_ms": position_ms},
                            "server_time_unix_ms": position_ms},
            "shows": [{"module": "dedications", "title": null, "color": "#c62828",
                       "small_widget": true, "closed": "message", "closed_message": null,
                       "page_url": null, "enabled": true, "hours": hours,
                       "timezone": "UTC", "state": null}]}]}))
        .unwrap()
    }

    #[test]
    fn widgets_hear_of_changes_only() {
        let gateway =
            Gateway::with_proofs(Config::default(), Vec::new(), Proofs::with_key([1; 32], 0))
                .unwrap();
        // Monday 9 November 2026.
        let at = |text: &str| text.parse::<Timestamp>().unwrap();
        let hours = json!([{"days": [1], "start_hour": 17, "end_hour": 18}]);
        gateway.apply(
            Some(state(hours.clone(), 1_000)),
            at("2026-11-09T16:59:58Z"),
        );
        let mut events = gateway.subscribe("main").unwrap();
        let first: Value = serde_json::from_str(&events.borrow_and_update()).unwrap();
        assert_eq!(first["shows"][0]["open"], false);
        // The position moves, nothing else: no event.
        gateway.apply(
            Some(state(hours.clone(), 2_000)),
            at("2026-11-09T16:59:59Z"),
        );
        assert!(!events.has_changed().unwrap());
        // The API does not answer: the show opens on time all the same.
        gateway.apply(None, at("2026-11-09T17:00:00Z"));
        assert!(events.has_changed().unwrap());
        let opened: Value = serde_json::from_str(&events.borrow_and_update()).unwrap();
        assert_eq!(opened["shows"][0]["open"], true);
        assert_eq!(
            opened["shows"][0]["closes_at_unix_ms"],
            at("2026-11-09T18:00:00Z").as_millisecond()
        );
        // A station gone: its widgets' streams end.
        gateway.apply(
            Some(ApiState {
                stations: Vec::new(),
            }),
            at("2026-11-09T17:00:01Z"),
        );
        assert!(gateway.subscribe("main").is_none());
        assert!(events.has_changed().is_err());
    }

    #[test]
    fn the_listeners_address_comes_from_the_proxy_when_told() {
        let peer = IpAddr::from([10, 0, 0, 1]);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "198.51.100.7, 203.0.113.9".parse().unwrap(),
        );
        let direct =
            Gateway::with_proofs(Config::default(), Vec::new(), Proofs::with_key([1; 32], 0))
                .unwrap();
        assert_eq!(
            direct.client(&headers, peer),
            peer,
            "a header is not trusted unless told"
        );
        let config = Config {
            client_ip_header: Some(HeaderName::from_static("x-forwarded-for")),
            ..Config::default()
        };
        let proxied =
            Gateway::with_proofs(config, Vec::new(), Proofs::with_key([1; 32], 0)).unwrap();
        assert_eq!(
            proxied.client(&headers, peer),
            IpAddr::from([203, 0, 113, 9])
        );
        headers.insert("x-forwarded-for", "garbage".parse().unwrap());
        assert_eq!(proxied.client(&headers, peer), peer);
    }

    #[test]
    fn writes_come_from_the_stations_websites() {
        let config = Config {
            allowed_origins: vec!["https://radio.example.org/".to_owned()],
            ..Config::default()
        };
        let gateway =
            Gateway::with_proofs(config, Vec::new(), Proofs::with_key([1; 32], 0)).unwrap();
        assert!(gateway.origin_allowed(Some("https://RADIO.example.org")));
        assert!(!gateway.origin_allowed(Some("https://elsewhere.example")));
        assert!(gateway.origin_allowed(None));
        let open =
            Gateway::with_proofs(Config::default(), Vec::new(), Proofs::with_key([1; 32], 0))
                .unwrap();
        assert!(open.origin_allowed(Some("https://elsewhere.example")));
    }
}
