# Monitoring

How to watch a Heron station from your own **Grafana** (self-hosted or Grafana Cloud): sending its metrics to Prometheus, importing the dashboard and the alert rules, health checks, logs, and error reports in **Sentry**.

Heron does not run Prometheus or Grafana itself. It provides:

| | Where | Details |
|---|---|---|
| Metrics, pushed | Every Rust service sends its metrics to a Prometheus-compatible database with remote write, when `HERON_METRICS_REMOTE_WRITE_URL` is set | [Sending the metrics](#sending-the-metrics) |
| Metrics, scraped | `/metrics` on every Rust service (Prometheus text format) | [Scraping instead](#scraping-instead) |
| Dashboard | [`monitoring/grafana/heron-dashboard.json`](../monitoring/grafana/heron-dashboard.json), to import | [The dashboard](#the-dashboard) |
| Alert rules | [`monitoring/prometheus/alerts.yml`](../monitoring/prometheus/alerts.yml), to load | [Alerts](#alerts) |
| Health | `/healthz` on every service, used by Docker's health checks | [Health checks](#health-checks) |
| Logs | Standard output of each container, one JSON object per line | [Logs](#logs) |
| Errors | Sentry, when `HERON_SENTRY_DSN` is set | [Sentry](#sentry) |

## Quick start with Grafana Cloud

1. In your Grafana Cloud stack, open **Prometheus → Details** (*Connections → Data sources → grafanacloud-…-prom* in recent stacks). Note the **remote write endpoint** (`https://prometheus-prod-….grafana.net/api/prom/push`) and the **username / instance ID** (a number).
2. Create an **access policy token** with the `metrics:write` scope (*Administration → Cloud access policies*, or *Generate now* on the same page).
3. In the station's `.env`:

   ```sh
   HERON_STATION_NAME=radio1
   HERON_METRICS_REMOTE_WRITE_URL=https://prometheus-prod-XX-prod-REGION.grafana.net/api/prom/push
   HERON_METRICS_REMOTE_WRITE_USERNAME=123456
   HERON_METRICS_REMOTE_WRITE_PASSWORD=glc_…
   ```

4. `docker compose up -d`. Each service logs `pushing metrics with remote write`.
5. In Grafana: *Dashboards → New → Import*, upload `monitoring/grafana/heron-dashboard.json`, and pick your Prometheus data source ([details](#importing-the-dashboard)).
6. Load the alert rules ([Alerts](#loading-the-alert-rules)) and set up a contact point.

## Sending the metrics

### Remote write

When `HERON_METRICS_REMOTE_WRITE_URL` is set, every Rust service (`heron-engine`, `heron-api`, `heron-analyzer`, `heron-librarian`) sends its metrics there every `HERON_METRICS_REMOTE_WRITE_INTERVAL` seconds, with [Prometheus remote write](https://prometheus.io/docs/specs/prw/remote_write_spec/) 1.0 (protobuf, snappy). Nothing needs to reach the station from outside: it only makes outgoing HTTPS requests, so it works behind NAT and firewalls.

| Variable | Default | Meaning |
|---|---|---|
| `HERON_METRICS_REMOTE_WRITE_URL` | empty: off | The remote write endpoint |
| `HERON_METRICS_REMOTE_WRITE_USERNAME` | | Basic authentication user (Grafana Cloud: the instance ID). Empty: the password is sent as a bearer token |
| `HERON_METRICS_REMOTE_WRITE_PASSWORD` | | Basic authentication password, or bearer token |
| `HERON_METRICS_REMOTE_WRITE_INTERVAL` | `15` | Seconds between pushes |
| `HERON_STATION_NAME` | `heron` | The `station` label of every metric: give each station its own name |

Every series carries `job` (the service: `heron-engine`, `heron-api`…) and `station`. The dashboard and the alert rules rely on both.

Where it can go:

| Receiver | URL |
|---|---|
| Grafana Cloud | `https://prometheus-prod-….grafana.net/api/prom/push`, with the instance ID and an access policy token |
| Prometheus 2.33 or later | `http://prometheus:9090/api/v1/write`, Prometheus started with `--web.enable-remote-write-receiver` |
| Grafana Mimir | `http://mimir:8080/api/v1/push` (add `X-Scope-OrgID` through a proxy if multi-tenant) |
| VictoriaMetrics | `http://victoriametrics:8428/api/v1/write` |
| Thanos Receive | `http://thanos-receive:19291/api/v1/receive` |

A push that fails is not retried: counters are totals, so the next push carries what was missed, and only graph resolution is lost during an outage. The first failure of an outage is logged as a warning (`cannot push metrics`), and recovery as `pushing metrics again`; `heron_remote_write_failures_total` counts failures. A wrong setting never stops a service: the engine keeps playing and logs `metrics are not pushed`.

### Scraping instead

If you prefer Prometheus to fetch the metrics, leave `HERON_METRICS_REMOTE_WRITE_URL` empty and scrape `/metrics` on each service:

| Service | Port | Scrape interval |
|---|---|---|
| `heron-engine` | 8081 | 5 s (short outages show) |
| `heron-api` | 8080 | 15 s |
| `heron-analyzer` | 8082 | 15 s |
| `heron-librarian` | 8083 | 15 s |

[`monitoring/prometheus/scrape.yml`](../monitoring/prometheus/scrape.yml) has the jobs to add to `prometheus.yml`. Keep the job names, and set the `station` label on the targets.

These ports have no authentication. A Prometheus in another Compose project can join Heron's network (`heron_default`):

```yaml
# In the other project's docker-compose.yml
services:
  prometheus:
    networks: [default, heron]
networks:
  heron:
    name: heron_default
    external: true
```

Otherwise, publish them with a `docker-compose.override.yml` next to `docker-compose.yml`, on `127.0.0.1` or a private network (VPN, LAN) only, never on the Internet:

```yaml
services:
  heron-engine:
    ports: ["127.0.0.1:9181:8081"]
  heron-api:
    ports: ["127.0.0.1:9180:8080"]
  heron-analyzer:
    ports: ["127.0.0.1:9182:8082"]
  heron-librarian:
    ports: ["127.0.0.1:9183:8083"]
```

## The dashboard

### Importing the dashboard

1. In Grafana (12 or later, or Grafana Cloud): *Dashboards → New → Import*.
2. Upload [`monitoring/grafana/heron-dashboard.json`](../monitoring/grafana/heron-dashboard.json), or paste its content, and choose a folder.
3. Open it and pick your Prometheus data source in the **Prometheus** drop-down at the top; every panel follows it. The **Station** drop-down lists the stations sending metrics (all by default).

The dashboard's UID is `heron-station`: importing a newer version replaces it (Grafana asks first). To keep your own changes, save a copy under another name.

Provisioning it from files instead (self-hosted Grafana): put the JSON in a directory read by a [dashboard provider](https://grafana.com/docs/grafana/latest/administration/provisioning/#dashboards).

### What it shows

| Row | Panels |
|---|---|
| **On air** | Whether the engine sends metrics, dead air, how long the master bus has been silent, outputs connected, tracks queued, server clock synchronized. Master level, each output's bitrate, its listeners and the total, fallback tracks and silence watchdog skips per minute, pause and queue length. |
| **Audio engine health** | Underruns (decoding fell behind), audio dropped by the engine, an encoder or an output, output reconnections, the pacing clock's lateness and drift, stalls, tracks that could not be preloaded. |
| **Server clock** | The kernel's clock offset, estimated error and synchronization. |
| **Control plane** | heron-api and its link to the engine, web clients, the analyzer and the librarian, library size, tracks queued by rotation and picks with relaxed rules, sign-ins. |
| **Library** | Analysis jobs by result and time per track, imports and library scans. |
| **Versions** | Every running service and its version, per station. |

Healthy: *Dead air* OK, *Silence* under a few seconds (pauses aside), every output connected, *Queue* 2 or more, *Clock drift* flat at 50 ms, underruns and dropped audio at zero, fallback tracks only when you expect them.

## Alerts

### Loading the alert rules

[`monitoring/prometheus/alerts.yml`](../monitoring/prometheus/alerts.yml) is a standard Prometheus rule file. Check it with `promtool check rules alerts.yml`.

- **Grafana Cloud, Mimir:** load it into the data source's ruler with [`mimirtool`](https://grafana.com/docs/mimir/latest/manage/tools/mimirtool/):

  ```sh
  mimirtool rules load monitoring/prometheus/alerts.yml \
    --address=https://prometheus-prod-XX-prod-REGION.grafana.net \
    --id=123456 --key=glc_…   # a token with the rules:write scope
  ```

  The rules then show in Grafana under *Alerting → Alert rules*, with the data source. They notify through Grafana Cloud's Alertmanager: set a contact point and a notification policy in *Alerting*. You can also recreate some of them as Grafana-managed rules (*Alerting → Alert rules → New*), using their expressions.
- **Prometheus:** list the file under `rule_files` in `prometheus.yml`, and point `alerting` at your Alertmanager.

Every rule works with pushed and scraped metrics, and alerts name the station.

### The rules

**Critical**: listeners hear the problem, or soon will. **Warning**: the air is safe, something needs a look.

| Alert | Severity | Fires when | What to do |
|---|---|---|---|
| `HeronEngineDown` | critical | An engine that sent metrics in the last 10 min sent none for 1 min, for 1 min | The station is probably off air, or its network is down. `docker compose ps heron-engine`, `docker compose logs heron-engine`. The engine exits by itself when its audio thread is stuck and Docker restarts it: repeated restarts point at the machine. |
| `HeronDeadAir` | critical | Nothing on air and nothing to play for 30 s | Put tracks in the fallback directory (`HERON_FALLBACK_DIR`). Check heron-api and that rotation can pick tracks (Rotation page). |
| `HeronSilenceOnAir` | critical | The master bus is silent for 45 s, not paused | The silence watchdog is off, or every queued and fallback track is silent. |
| `HeronOutputDisconnected` | critical | An output is not connected for 2 min | Check the stream server and the output's host, mount and password (Outputs page). The engine keeps retrying. |
| `HeronAudioDropped` | critical | The engine dropped audio in the last 10 min | The machine is too busy for the encoders: fewer or cheaper encoder profiles, or a faster machine. |
| `HeronFallbackPlaying` | warning | A fallback track started in the last 10 min | The queue ran dry: heron-api down or cut from the engine, autofill off, or restrictions forbid every track. |
| `HeronSilenceSkips` | warning | The watchdog skipped a track in the last 30 min | The engine's logs name the track (`silence on air`): check its file and its analysis. |
| `HeronUnderruns` | warning | More than 50 blocks without decoded audio in 10 min | The machine is overloaded, or the library is slow (network share). |
| `HeronOutputUnstable` | warning | An output reconnected more than 3 times in 30 min | Network or stream server trouble. |
| `HeronPreloadFailures` | warning | Queued tracks could not be read | The library is unreachable, or files were deleted. |
| `HeronTrackStalled` | warning | A cued track could not start within 10 s while nothing played, and was skipped | Its file hangs: an unreachable network share. The fallback directory filled in. |
| `HeronClockStalled` | warning | The engine stalled for more than 1 s in the last hour | The machine was suspended or starved of CPU: listeners heard a gap. |
| `HeronClockNotSynchronized` | warning | The kernel reports the clock unsynchronized for 10 min | Run NTP on the host (chrony, systemd-timesyncd). |
| `HeronClockOff` | warning | The clock may be off by more than 1 s for 10 min | Same. |
| `HeronServiceDown` | warning | heron-api, heron-analyzer or heron-librarian sent no metrics for 1 min, for 2 min | The air stays on; the station cannot be controlled or fed. |
| `HeronApiLostEngine` | warning | heron-api cannot reach the engine for 2 min | The queue is not refilled; the fallback directory takes over when it runs dry. |
| `HeronAnalysisFailing` | warning | More than 10 analyses failed in an hour | Library page, heron-analyzer's logs. |
| `HeronLibraryScanFailing` | warning | A library scan failed in the last 2 h | Is the music directory mounted and readable? |
| `HeronLoginFailures` | warning | More than 20 failed sign-ins in 10 min | Someone may be guessing passwords. |

Thresholds are starting points: edit them to suit the station. `HeronEngineDown` and `HeronServiceDown` only know a station that has sent metrics in the last 10 minutes: a station that never started is not reported.

## Metrics reference

Every service also exports `heron_build_info{service, version}` (always 1), and, when pushing, `heron_remote_write_failures_total`. Pushed series also carry `job` and `station`.

### heron-engine

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `heron_engine_rendered_seconds` | gauge | | Audio rendered since the engine started |
| `heron_engine_clock_drift_seconds` | gauge | | Audio rendered minus time elapsed: constant (the 50 ms lead) while the clock keeps time |
| `heron_engine_clock_max_late_seconds` | gauge | | Worst lateness of a 10 ms block over the last second |
| `heron_engine_clock_resets_total` | counter | | Stalls longer than 1 s (machine suspended, CPU starved) |
| `heron_engine_underruns_total` | counter | | 10 ms blocks where a deck had no decoded audio |
| `heron_engine_underrun_tracks_total` | counter | | Tracks that had at least one underrun |
| `heron_engine_dropped_blocks_total` | counter | | 10 ms blocks the encoders did not take in time (lost audio) |
| `heron_engine_mixer_overflows_total` | counter | | Mixer events the player could not take in time (never expected) |
| `heron_engine_paused` | gauge | | 1 while paused |
| `heron_engine_master_peak` | gauge | | Peak level of the master bus, linear (1 = full scale); in dBFS: `20 * log10(...)` |
| `heron_engine_silent_seconds` | gauge | | How long the master bus has been below −50 dBFS (0 when there is sound) |
| `heron_engine_queue_length` | gauge | | Tracks waiting after the cued one |
| `heron_engine_dead_air` | gauge | | 1 when nothing is on air and there is nothing to play |
| `heron_engine_fallback_tracks_total` | counter | | Tracks started from the fallback directory |
| `heron_engine_silence_skips_total` | counter | | Tracks skipped by the silence watchdog |
| `heron_engine_preload_failures_total` | counter | | Queued tracks whose file could not be read ahead of time |
| `heron_engine_stalled_tracks_total` | counter | | Cued tracks skipped because they could not start within 10 s while nothing played |
| `heron_clock_synchronized` | gauge | | 1 when the kernel reports the clock synchronized (Linux only) |
| `heron_clock_offset_seconds` | gauge | | Offset the kernel is correcting |
| `heron_clock_estimated_error_seconds` | gauge | | The kernel's estimate of the clock's error |
| `heron_output_connected` | gauge | `output` | 1 while the output streams |
| `heron_output_bytes_total` | counter | `output` | Bytes sent to the stream server |
| `heron_output_reconnects_total` | counter | `output` | Reconnections |
| `heron_output_dropped_chunks_total` | counter | `output` | Encoded audio dropped because the output was behind |
| `heron_encoder_dropped_chunks_total` | counter | `profile` | Audio dropped because the encoder was behind |

### heron-api

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `heron_api_engine_connected` | gauge | | 1 while the API is connected to the engine |
| `heron_api_engine_disconnections_total` | counter | | Connections to the engine lost |
| `heron_api_live_clients` | gauge | | Web clients on the live WebSocket |
| `heron_api_autofill_tracks_total` | counter | | Tracks queued by autofill (rotation) |
| `heron_api_rotation_relaxed_total` | counter | | Picks that needed relaxed rules |
| `heron_api_logins_total` | counter | `result` (`ok`, `failed`) | Sign-ins |
| `heron_listeners` | gauge | `output` | Listeners of the output at the last minute, from its stream server; `NaN` while the server cannot be read ([Listener statistics](statistics.md)) |
| `heron_api_listener_polls_total` | counter | `result` (`ok`, `error`) | Requests to stream servers for listener statistics |
| `heron_api_backup_last_success_timestamp_seconds` | gauge | | When the last backup that passed its checks was taken |
| `heron_api_backup_last_test_success_timestamp_seconds` | gauge | | When a backup last passed a [test read](backups.md#testing-backups) |
| `heron_api_backups_damaged` | gauge | | Backups whose files failed the daily checksum check |
| `heron_library_files` | gauge | | Audio files found by the last library scan |
| `heron_library_scan_duration_seconds` | gauge | | Duration of the last scan |
| `heron_library_scans_total` | counter | `result` (`ok`, `error`) | Library scans |

### heron-analyzer

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `heron_analysis_jobs_total` | counter | `result` (`ok`, `retry`, `failed`, `error`) | Analysis jobs finished |
| `heron_analysis_seconds` | summary | `quantile` | Time to analyze a track |
| `heron_analysis_audio_ms_total` | counter | | Audio analyzed, in ms |

### heron-librarian

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `heron_librarian_imports_total` | counter | `result` (`identified`, `needs_review`, `unidentified`, `failed`) | Files imported from the incoming folder |

Counters that only grow when something happens (`heron_engine_silence_skips_total`…) start at 0 when the engine starts, so `increase()` works from the first sample.

## Health checks

Every service answers `GET /healthz` on its port:

```json
{"service": "heron-engine", "version": "0.1.0", "status": "ok"}
```

- **200** with `"status": "ok"`: healthy. Docker's health checks (in the images) use it; `docker compose ps` shows the result.
- **503** with `"status": "unavailable"` and a `reason`: unhealthy (the engine's audio thread stopped rendering, the API lost its database…).
- **`warnings`**: problems that need attention without making the service unhealthy. The engine lists:
  - `the server clock is not synchronized (is NTP running on the host?)` or `the server clock may be off by 2.5 s`;
  - `nothing to play: the queue is empty and the fallback directory has no track`.

Every service restarts by itself (`restart: unless-stopped`). Docker does not restart a container only because it is unhealthy, so the engine exits on its own when its audio thread is stuck for 10 s: Docker then restarts it, and it resumes the queue it saved.

From outside, watch what listeners get: an HTTP check of the stream URL (`https://stream.example.org/live` answering `200`) in Grafana Cloud's Synthetic Monitoring, [Uptime Kuma](https://github.com/louislam/uptime-kuma) or UptimeRobot. It catches what the station cannot see from inside (DNS, TLS, the network, the stream server), and a station that stopped sending metrics altogether.

## Logs

Containers log one JSON object per line (`HERON_LOG_FORMAT=json`), with `timestamp`, `level`, `target`, `message` and the event's fields:

```json
{"timestamp":"2026-10-18T19:26:47.99Z","level":"WARN","message":"silence on air: skipping the track","entry_id":42,"path":"\"Artist/Album/01 Track.mp3\"","seconds":20,"to_fallback":false,"target":"heron_engine::player"}
```

The level is set with `RUST_LOG` in `.env` (`info` by default; `heron_engine=debug,info` for more from the engine). `docker compose logs -f heron-engine` follows one service.

To search them in Grafana, send them to Loki (Grafana Cloud Logs or your own) with [Grafana Alloy](https://grafana.com/docs/alloy/latest/) running next to the station. A `config.alloy`:

```alloy
discovery.docker "containers" {
  host = "unix:///var/run/docker.sock"
}

discovery.relabel "heron" {
  targets = discovery.docker.containers.targets
  rule {
    source_labels = ["__meta_docker_container_label_com_docker_compose_service"]
    target_label  = "service"
  }
  rule {
    target_label = "station"
    replacement  = "radio1"
  }
}

loki.source.docker "heron" {
  host       = "unix:///var/run/docker.sock"
  targets    = discovery.relabel.heron.output
  forward_to = [loki.write.default.receiver]
}

loki.write "default" {
  endpoint {
    // Grafana Cloud: Loki → Details; your own: http://loki:3100/loki/api/v1/push
    url = "https://logs-prod-XX.grafana.net/loki/api/v1/push"
    basic_auth {
      username = "654321"
      password = sys.env("GRAFANA_CLOUD_TOKEN")
    }
  }
}
```

with Alloy in a `docker-compose.override.yml`, the Docker socket mounted read-only:

```yaml
services:
  alloy:
    image: grafana/alloy:v1.11.0
    restart: unless-stopped
    command: ["run", "/etc/alloy/config.alloy", "--storage.path=/var/lib/alloy"]
    environment:
      GRAFANA_CLOUD_TOKEN: ${GRAFANA_CLOUD_TOKEN}
    volumes:
      - ./config.alloy:/etc/alloy/config.alloy:ro
      - /var/run/docker.sock:/var/run/docker.sock:ro
```

Useful queries:

```logql
{service="heron-engine"} | json | level="ERROR"
{service="heron-engine"} |= "silence on air"
{service=~"heron-.*"} | json | level=~"WARN|ERROR"
```

## Sentry

Errors and panics of every service, and JavaScript errors of the web interface, can be reported to [Sentry](https://sentry.io) or a self-hosted Sentry. It is off unless a DSN is set.

| Variable | Used by | Meaning |
|---|---|---|
| `HERON_SENTRY_DSN` | Rust services | DSN of the project receiving the services' errors |
| `HERON_WEB_SENTRY_DSN` | `heron-web` | DSN of the project receiving the browser's errors |
| `HERON_SENTRY_ENVIRONMENT` | both | Environment shown in Sentry (`production` by default) |
| `HERON_SENTRY_TRACES_SAMPLE_RATE` | Rust services | Share of operations traced for performance monitoring, 0 (default) to 1 |

None of them can stop a service: an invalid value is logged as an error and ignored. The same goes for the remote write settings (an invalid interval means 15 s).

1. In Sentry, create a project for the services (platform *Rust*) and, if you want browser errors, another for the web interface (platform *React*). Browsers can read the web DSN, so keep it separate from the services'.
2. Copy their DSNs into `.env`:

   ```sh
   HERON_SENTRY_DSN=https://<key>@o123456.ingest.sentry.io/1234567
   HERON_WEB_SENTRY_DSN=https://<key>@o123456.ingest.sentry.io/7654321
   HERON_SENTRY_ENVIRONMENT=production
   ```

3. `docker compose up -d`. Each service logs `reporting errors to Sentry` at startup; an invalid DSN is logged as an error and the service runs without Sentry.

What is sent:

- **Rust services:** every `ERROR` log becomes a Sentry event, with the `WARN` logs before it as breadcrumbs, and every panic. `INFO` logs are not sent. Fields that may identify people (`user`, `username`, `by`, `email`, addresses, passwords, tokens, keys) are replaced by `[redacted]` in events and breadcrumbs. Events carry the release (`heron-engine@0.1.0`…), a `service` tag and the host's name. No IP addresses or request bodies. The engine's audio thread never logs, so reporting never touches it.
- **Web interface:** uncaught errors and unhandled promise rejections, with the release `heron-web@<version>` and a `service` tag. No user details, cookies, headers, request bodies or query strings (sessions and API tokens travel in them). The Sentry SDK is only downloaded by browsers when the DSN is set.

The web interface reads its DSN when the page loads, from `/config.js`, which `heron-web` builds from its environment at startup: changing it needs `docker compose up -d heron-web`, not a rebuild.

## What else could be monitored

Not provided yet, and worth adding for a station on air around the clock:

- **The host**: CPU, memory, disk, temperature (a Raspberry Pi throttles when hot). The web UI's [status bar](web.md#status-bar) shows the CPU, memory and load average to operators, as read by the API: inside a container, the CPU and memory can be the container's own and the load average is the host's, so it is a glance, not a replacement for monitoring the host. Grafana Alloy can collect them (`prometheus.exporter.unix`) and push them with the same remote write settings, as can [node_exporter](https://github.com/prometheus/node_exporter) when scraped.
- **The stream from outside**: see [Health checks](#health-checks).
- **Track changes as events**: webhooks on track start, end and skip ([M7](roadmap/m7-webhooks-output-api.md)) can post to a chat or annotate Grafana.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `cannot push metrics` in the logs | Wrong URL or credentials (the log gives the server's answer: `401`, `404`…), or no outgoing HTTPS from the station. Grafana Cloud needs the *instance ID* as user and a token with `metrics:write`. |
| A self-hosted Prometheus answers `404` to pushes | Start it with `--web.enable-remote-write-receiver`. |
| The *Station* drop-down is empty | No metrics yet, or they lack the `station` label (scraping without the labels of `scrape.yml`). |
| Panels say *No data* | Pick the right data source at the top of the dashboard. Output panels stay empty until an output is configured. |
| Clock panels are empty | The clock state is read from the Linux kernel (in containers, the host's or Docker Desktop's VM's): nothing when the engine runs natively on macOS or Windows. |
| `HeronEngineDown` never fires for a station | It only covers stations that sent metrics in the last 10 minutes; watch the stream from outside too. |
| Nothing in Sentry | Check the DSN, and that the service logged `reporting errors to Sentry`; the station needs outgoing HTTPS. |
