# M3 – 24/7 reliability

**Depends on:** M2

## Scope

Already delivered in [M2](m2-engine-core.md): the queue lives in the engine's memory and tracks are decoded 10 s ahead, the engine never touches the database, outputs reconnect with exponential backoff, a cued file that cannot be decoded is skipped, and outputs and settings survive a restart. Left for this milestone:

- Fallback directory when the queue runs dry or the API is unreachable; silence watchdog on the output.
- Keeping more of the upcoming tracks ready (decoded or read into memory), so a slow or unreachable library does not cause underruns.
- Persist the queue and resume playback on restart.
- Docker restart policies and health checks.
- Clock sync monitoring in `heron-engine`: `heron_clock_synchronized` and `heron_clock_offset_seconds` in `/metrics`, and a warning in `/healthz` when the clock is not synced or is off by more than about 1 s (see [Requirements – Reliability](../requirements.md#5-reliability-247)). The reading itself (`heron_service::clock`) was added in [M5](m5-web-ui.md), whose dashboard shows the warning through `GET /api/v1/status`.
- Monitoring, added to the milestone on request, for the station's own Grafana (self-hosted or Grafana Cloud; Heron runs neither Prometheus nor Grafana): every service pushes its metrics with Prometheus remote write, and Heron ships a dashboard to import and alert rules to load, with a guide ([Monitoring](../monitoring.md)).
- Error reporting to Sentry from every Rust service and the web interface, off unless a DSN is set.

## What was built

- **Preloading:** the files of the next 3 queued tracks are read into memory ahead of time (`HERON_PRELOAD_TRACKS`); tracks then decode from memory and play to the end even if the library disappears.
- **Fallback directory:** cued when the queue runs dry (30 s before the track on air ends, or at once when nothing plays); files analyzed once in the background and cached in `fallback.json`; played in random order without repeats; anything queued replaces a cued fallback track. Autofill does not count fallback tracks.
- **Silence watchdog:** after `silence_watchdog.timeout_ms` (20 s, a playout setting editable on the Crossfade & loudness page) below −50 dBFS, skips to the next track, then to the fallback directory if the next is silent too. Dead air (nothing to play for 5 s) is logged as an error, exported and listed in `/healthz`.
- **Queue persistence:** `queue.json` saved on every change, every 5 s and at shutdown; a restart resumes the track on air at its position (fast skip to the position, 500 ms fade-in) and the queue, keeping entry IDs. On SIGTERM the queue is saved first and open gRPC streams get 2 s, so a restart takes about 2 s instead of Docker's 10 s timeout.
- **Stalled tracks:** a cued track that cannot start within 10 s while nothing plays (its file hangs on an unreachable share) is dropped and a fallback track plays.
- **Self-restart:** the engine exits when its audio thread is stuck for 10 s, so Docker's restart policy brings it back with its queue.
- **Priorities:** CPU weights between containers (engine 8192, analyzer and librarian 256), memory kill order (analyzer first, engine last), and background work lowered from inside: analyzer and librarian at nice 10 with the lowest best-effort I/O priority (`HERON_NICE`), the engine's fallback analysis thread at 19 ([Architecture – Priorities](../architecture.md#priorities)).
- **Health and metrics:** `/healthz` lists `warnings` (clock, dead air) without failing; new engine metrics for silence, dead air, fallback, watchdog skips, preload failures and the clock.
- **Remote write:** with `HERON_METRICS_REMOTE_WRITE_URL` set, every service pushes its metrics (remote write 1.0, basic or bearer authentication), labelled with `job` and `station`. A wrong setting is logged, never fatal. Every image now has CA certificates (HTTPS for remote write and Sentry).
- **Dashboard and alerts:** `monitoring/grafana/heron-dashboard.json` (data source and station selectors) and `monitoring/prometheus/alerts.yml` (19 rules, working with pushed or scraped metrics).
- **Tests:** player tests for the fallback directory, dead air, the watchdog (and pauses), and resuming after a restart; decoder tests for in-memory files and fast skipping; `scripts/chaos-test.sh`, also run in CI on amd64 and arm64.

## Known limitations

- Rotation picks restored after an engine restart are not linked to their plays in the rotation log (picks are keyed by the engine's start time and entry ID; the play history itself is complete).
- Fallback files are analyzed with the default analysis settings, not the station's (the engine never reads the database).
- The clock state is read from the Linux kernel only.

## Acceptance criteria

- [x] Chaos tests (`scripts/chaos-test.sh`, listening to the stream throughout) — in every case the stream continues or recovers automatically:
  - [x] stop MariaDB (90 s: no silence; the API recovers its database);
  - [x] stop the API (and empty the queue: the fallback directory plays, no silence; the API refills the queue when back);
  - [x] restart Icecast (the output reconnects within seconds);
  - [x] delete a queued file (the files of every waiting track: played from memory or skipped, no silence);
  - [x] inject a silent file (one 20 s silence, then the watchdog skips it).
- [x] An engine restart resumes playback within a few seconds (clean restart and kill: back on Icecast after 1–2 s, the track on air resumed at its position).
- [x] With time sync stopped on the host, `heron_clock_synchronized` drops to 0 and `/healthz` reports a clock warning (kernel status set to unsynchronized: metric 0, `/healthz` 200 with the warning; back to 1 once cleared).
- [x] Metrics pushed by remote write reach a Prometheus with every series labelled `job` and `station`; the dashboard imports into a separate Grafana (any data source name) and shows the station; the alert rules load, and `HeronEngineDown` fires 2 min after the engine stops and clears when it is back (`HeronFallbackPlaying` and `HeronServiceDown` fired during the chaos tests).
- [x] With a DSN set, an error logged by a service reaches Sentry with its release, `service` tag and breadcrumbs.

## References

- [Requirements – Reliability](../requirements.md#5-reliability-247)
- [Audio engine – Failure handling](../audio-engine.md#failure-handling)
