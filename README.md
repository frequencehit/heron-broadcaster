# Heron Broadcaster

A modern, self-hosted **radio automation system** for Linux. Heron plays music 24/7 to your stream servers, schedules tracks with category-based clockwheels, crossfades and removes silences, processes audio, and is operated entirely from a web interface.

The name comes from "air on" sounding like *héron*: keeping the air on, 24/7.

![The On air page: decks A and B with their categories, the next tracks, levels, the FX player counting down to a liner, outputs and the engine's state](docs/images/on-air.png)

> **Status:** early development, but it plays. Heron imports and analyzes a music library, takes new music by upload or through an incoming folder and files it once you import it, plays a queue with crossfades to Icecast servers, picks the music with categories, clocks and a weekly schedule, keeps the air on when the database, the API or the library goes away, and when Heron itself is upgraded, tells websites and bots what plays through signed webhooks and public now-playing endpoints, counts its listeners from the stream servers, shapes the sound with a deck EQ, and is operated from a web interface in English or French. Next: the AGC and multiband processing (M10). See the [roadmap](docs/roadmap/README.md).

Heron runs on Linux, amd64 and arm64, down to a Raspberry Pi.

## What it does today

- **Library:** watches your music folder (MP3, AAC and ALAC in M4A, FLAC, Ogg Vorbis, WAV, AIFF) and analyzes every track: loudness, true peak, silences at both ends, where the next track can start, a waveform. See [Import and analysis](docs/analysis.md).
- **New music and deletion:** files uploaded from the browser (resumable, folders included) or dropped into the incoming folder wait in a buffer zone, identified (AcoustID, MusicBrainz) and checked for duplicates and unplayable files, until you import them. There you fix their tags and give them categories, labels, mood and sound code, so they are ready to air once imported; they are then tagged and filed as `Artist/Album/01 Title.ext`. Deleted tracks go to a trash they can be restored from for 30 days. Reorganizing the library is a dry run first, journaled, and can be undone. See [Library management](docs/library.md).
- **Moving from another software:** an export of another radio automation software (titles, artists, categories, fades, intro and hook points, problems) is matched to the library by file name, checked in a dry run where conflicts with Heron's own values are settled, then applied as one import that can be undone. See [Importing from another software](docs/import.md).
- **Playout:** two decks with crossfades or fixed overlaps, Gap Killer, loudness normalization, an FX player for jingles over the music, a fallback folder, MP3 to one or more Icecast servers with track metadata, and any codec of the station's own `ffmpeg` (AAC+ with `libfdk_aac`, for instance) through the external FFmpeg encoder. See [Audio engine](docs/audio-engine.md) and [Encoders and outputs](docs/outputs.md).
- **Medleys:** the hooks of several songs strung together with the station's liners, between them or over the music, built from a recipe, adjusted on a timeline (moves, crossfades, a fade-out) and listened to before they are mixed into a track of the library, named after their artists or titles. See [Medleys](docs/medleys.md).
- **Deck EQ:** a 10-band EQ for the music and another for the FX player, ±12 dB, heard as the sliders move and without clicks, with presets. See [Audio engine – Deck EQ](docs/audio-engine.md#deck-eq).
- **Reliability and monitoring:** the next tracks are read into memory ahead of time, a fallback folder plays when the queue runs dry, a silence watchdog skips dead tracks, and the queue resumes where it was after a restart. Every service pushes its metrics to your Prometheus or Grafana Cloud, with a dashboard and alert rules to import, and errors can go to Sentry. See [Monitoring](docs/monitoring.md).
- **Upgrades on air:** a new engine takes over from the running one, with its queue and its connection to Icecast, so an upgrade costs listeners about 70 ms of silence instead of a disconnection; when the engine does restart, Icecast's fallback mount keeps them connected. See [Upgrading](docs/upgrading.md) and [Engine handover](docs/handover.md).
- **Rotation:** categories, clocks of slots, weekly schedules (a default one, and others for their periods: Christmas, the summer, every year if need be), separation rules (title, artist, album) relaxed step by step when the library runs short, artists with several names, restrictions (dayparting, date ranges, sequences) that are never relaxed, and a selection log that explains every pick. A simulation checks a day or a week ahead. See [Rotation](docs/rotation.md).
- **Backups:** every day, everything set by hand (stations, programming, outputs, users, and what is set on each track) to a configuration file, with a dump of the database, in a folder that can be a network share; a configuration file restores from the web interface on top of a library scanned again. See [Backups](docs/backups.md).
- **Several stations:** one Heron runs several stations that share the library. Each has its own engine (one more service of the deployment), programming, queue, outputs, history and webhooks; categories and medleys belong to one station or several, rotation rules can keep the stations apart, and users get a role on each station. See [Several stations](docs/stations.md).
- **Integrations:** webhooks on track start, end and skip, with Heron's JSON or a template of your own, signed (HMAC-SHA256), retried with backoff and logged; public now-playing and history endpoints for the station's website, readable from any origin. See [API – Webhooks](docs/api.md#webhooks).
- **Listener statistics:** listeners per day, hour, output and track, read from your Icecast servers once a minute: peak, average, listening hours, sessions, distinct listeners, the hours of the day, the players and the tracks listeners leave, with CSV exports. No IP address is stored. See [Listener statistics](docs/statistics.md).
- **Web interface:** on air, queue, history, statistics, tracks with their waveform, cue points, details, labels and tags, the buffer and the trash, artists, clocks, schedule, restrictions, the deck EQ, outputs, webhooks and settings; live updates, light and dark themes, English and French, from phone width up. Viewer, operator and admin roles. See [Web UI](docs/web.md).
- **API:** everything the web interface does goes through a documented HTTP and WebSocket API (OpenAPI), with API tokens for scripts. See [API and control plane](docs/api.md).

## Screenshots

| | |
|---|---|
| [![A clock: its slots, the hour drawn as it will sound, a simulation](docs/images/clock-editor.png)](docs/images/clock-editor.png) **Clock editor:** slots by drag and drop, the hour as it will sound | [![A track: waveform with cue-in, cue-out, the end of the intro, the hook and where the next track starts](docs/images/track.png)](docs/images/track.png) **Track:** waveform, cue points, intro and hook, crossfade, analysis |
| [![The week, hour by hour, painted with clocks](docs/images/schedule.png)](docs/images/schedule.png) **Schedule:** which clock runs at each hour of the week, and the schedules for seasons | [![The deck EQ: ten sliders and the response curve for the music, and for the FX player](docs/images/eq.png)](docs/images/eq.png) **Deck EQ:** the music and the FX player, heard as the sliders move |
| [![A medley: its hooks and liners on a timeline, the liners over the music, the fade-out, the playhead](docs/images/medley.png)](docs/images/medley.png) **Medleys:** hooks and liners mixed into a track, on a timeline to listen to and adjust | [![The buffer: an album waiting to be imported, with its paths and categories](docs/images/buffer.png)](docs/images/buffer.png) **Buffer:** new music identified, checked and prepared before it is imported |
| [![The selection log: why each track was picked, from which clock and slot, the rules held or relaxed](docs/images/selection-log.png)](docs/images/selection-log.png) **Selection log:** why each track was picked, and a simulation of the day or the week | [![The outputs: an Icecast stream connected with its listeners, the encoder profiles](docs/images/outputs.png)](docs/images/outputs.png) **Outputs:** stream servers and encoders, live state and listeners |
| [![The stations, each with the variable of its engine's address](docs/images/stations.png)](docs/images/stations.png) **Several stations:** each with its own engine, chosen at the top of the menu | [![The categories, with badges for the stations each one belongs to](docs/images/categories-stations.png)](docs/images/categories-stations.png) **Categories per station:** the library is shared, each category belongs to one station or several |
| [![The rotation settings of a station, with separation rules across stations](docs/images/rules-across-stations.png)](docs/images/rules-across-stations.png) **Rules across stations:** keep an artist or a track apart from what the other stations play | [![The users, with a shared role and a role on each station](docs/images/roles-per-station.png)](docs/images/roles-per-station.png) **Roles per station:** operator on one station, viewer on another, or no access |
| [![The On air page in the dark theme](docs/images/on-air-dark.png)](docs/images/on-air-dark.png) **Dark theme** | <p align="center"><img src="docs/images/on-air-phone.png" alt="The On air page at phone width" width="200"></p> **Phone width** |

The screenshots show demo stations with generated music by fictional artists.

## Quick start

Requirements: Docker with Compose v2.

```sh
cp .env.example .env              # then change the passwords
mkdir -p data/music data/fallback data/incoming data/staging data/backups # or point HERON_MUSIC_DIR, … elsewhere
docker compose up -d --wait       # pulls the images from ghcr.io/dehy
docker compose exec heron-api heron-api user add admin --role admin   # asks for a password
```

Open http://localhost:8000 and sign in. Database migrations run automatically when `heron-api` starts.

The images are built for amd64 and arm64 (Raspberry Pi), and published by CI once the whole stack has passed its tests on both. To build them from source instead, add `--build`.

### Choosing a version

The services run the `latest` release by default; `edge` follows `main`. To pin a release, create a `docker-compose.override.yml` next to `docker-compose.yml` (Compose reads it automatically, and git ignores it):

```yaml
services:
  heron-api:
    image: ghcr.io/dehy/heron-api:0.1.0
  heron-analyzer:
    image: ghcr.io/dehy/heron-analyzer:0.1.0
  heron-librarian:
    image: ghcr.io/dehy/heron-librarian:0.1.0
  heron-engine:
    image: ghcr.io/dehy/heron-engine:0.1.0
  heron-web:
    image: ghcr.io/dehy/heron-web:0.1.0
```

Then `docker compose pull && docker compose up -d --wait`. Setting `HERON_VERSION=0.1.0` in `.env` does the same for all five services at once. A fork's CI publishes its own images, under `ghcr.io/<owner>`: set `HERON_REGISTRY=ghcr.io/<owner>` in `.env` to run them. To upgrade, change the version and run the same two commands; migrations run when `heron-api` starts. Restarting the engine this way leaves a few seconds of fallback on the stream; `scripts/upgrade.sh` instead hands the stream over from the running engine to the new one, without a gap for the listeners. See [Upgrading](docs/upgrading.md), for Compose and Swarm.

Point `HERON_MUSIC_DIR` at your music: new and changed files are picked up automatically, and the **Tracks** page shows them as they are analyzed. See [Import and analysis](docs/analysis.md).

Upload new files from the web interface (*Buffer*), or drop them into `HERON_INCOMING_FOLDER` (`data/incoming`): they wait in the buffer zone (`HERON_STAGING_FOLDER`, `data/staging`), identified by `heron-librarian` (with your station's AcoustID key, in **Settings**) and checked for duplicates, until you import them. Edit their tags and put them in categories there, before the import; they are then tagged and filed into the library as `Artist/Album/01 Title.ext`. Deleted tracks go to a trash they can be restored from for 30 days. Heron never overwrites a file, and reorganizes your existing library only when you ask, after a dry run. Set `HERON_LIBRARIAN_USER` to the owner of your music (`id -u`:`id -g`), the only user Heron writes as. See [Library management](docs/library.md).

To try it without your own music, generate a synthetic library (uses ffmpeg in Docker):

```sh
scripts/generate-test-library.sh data/music 100
```

### Going on air

Heron starts playing as soon as tracks are analyzed: with autofill on (the default), the queue manager keeps three tracks queued, picked by rotation. To hear it, start the local Icecast server of the `icecast` profile:

```sh
docker compose --profile icecast up -d --wait
```

Then, in the web interface, open **Outputs** and add an Icecast output: host `icecast`, port `8000`, mount `/live`, password `hackme`, public URL `http://localhost:8010`, with the MP3 profile. Click **Listen** or open http://localhost:8010/live.

To shape the programme, create **Categories**, put tracks in them (from the track's page), build **Clocks** from them and paint the week in **Schedule**. Hours without a clock rotate the whole library. A season of its own (Christmas, the summer) is another schedule, with its periods.

The same can be done from scripts: every route is in [API and control plane](docs/api.md) and in the OpenAPI document at http://localhost:8000/api/v1/openapi.json.

```sh
api=http://localhost:8000/api/v1
curl -c cookies -H 'Content-Type: application/json' -d '{"username": "admin", "password": "…"}' $api/auth/login
curl -b cookies $api/player
curl -b cookies -X POST -H 'X-Requested-With: curl' $api/player/skip
```

### Services

| Service | Role | Port (inside the network) |
|---|---|---|
| `heron-web` | Web interface, proxies `/api` to `heron-api` | 80, published on `HERON_HTTP_PORT` (8000) |
| `heron-api` | HTTP and WebSocket API, migrations, library scanner, queue manager, rotation, uploads | 8080 |
| `heron-librarian` | Incoming folder, buffer zone, identification, tags, library layout, trash | 8083 |
| `heron-analyzer` | Track analysis workers | 8082 |
| `heron-engine` | Real-time playout, encoders, outputs | 8081, gRPC control on 50051 |
| `heron-relay` | Holds the engine's connections to stream servers, so an upgrade hands them to the new engine ([Upgrading](docs/upgrading.md)) | Unix socket in the `engine-relay` volume |
| `mariadb` | Database | 3306 |
| `icecast` | Optional local Icecast server (`--profile icecast`), `/live` falling back to `/fallback` | 8000, published on `HERON_ICECAST_PORT` (8010) |
| `icecast-fallback` | Loops an MP3 (silence by default) on Icecast's `/fallback` (`--profile icecast`) | none |

Every Rust service serves `/healthz` and `/metrics` (Prometheus) on its port. With `HERON_METRICS_REMOTE_WRITE_URL` set, they also push their metrics to your Prometheus or Grafana Cloud (remote write); import `monitoring/grafana/heron-dashboard.json` into Grafana and load the alert rules of `monitoring/prometheus/alerts.yml`. `HERON_SENTRY_DSN` reports errors to Sentry. See [Monitoring](docs/monitoring.md).

Put a few tracks in the fallback directory (`HERON_FALLBACK_DIR`, `./data/fallback` by default): the engine plays them when the queue runs dry, for example while the API is down.

## Development

Requirements: Rust (the toolchain in `rust-toolchain.toml` installs automatically through rustup), Node.js 22, Docker.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Database tests create and drop scratch databases, so they need an admin URL:
docker run -d --rm --name heron-test-db -p 127.0.0.1:33306:3306 \
  -e MARIADB_ROOT_PASSWORD=root mariadb:11.8
HERON_TEST_DATABASE_URL=mysql://root:root@127.0.0.1:33306 cargo test --workspace

# Soak test of the engine: two Icecast servers, the library for an hour:
cargo build --release -p heron-engine
scripts/soak-test.sh data/music 3600

# Chaos tests (M3) on a running stack with tracks in the library and in
# the fallback directory: stops the database and the API, restarts Icecast
# and the engine, deletes queued files, queues a silent file:
docker compose --profile icecast up -d --wait
scripts/chaos-test.sh

# Analyze files with given settings, without a database:
HERON_ANALYSIS_SETTINGS='{"silence_ceiling_dbfs": -60}' \
  cargo run --release -p heron-analysis --example analyze -- path/to/*.mp3

# Web app, with /api proxied to a local heron-api on :8080:
cd web && npm ci && npm run dev
npm test && npm run typecheck   # unit tests (translation catalogs included) and types
```

To run your changes in Docker Compose, build the images: `docker compose up -d --build --wait`. Running a binary locally: `cargo run -p heron-api -- --help`. All options can also be set through `HERON_*` environment variables. Adding a language to the web interface takes one catalog file: see [Web UI – Adding a language](docs/web.md#adding-a-language).

## Documentation

Start with [docs/README.md](docs/README.md): requirements, architecture, data model, conventions and the milestone roadmap.

## Built with Claude

Heron Broadcaster is written with [Claude](https://claude.com/claude-code), Anthropic's AI coding assistant. This is a deliberate choice, and we stand behind it openly. A human maintainer sets the direction and makes the decisions ([docs/decisions.md](docs/decisions.md)), and reviews every change. Commits written with Claude carry a `Co-Authored-By: Claude` trailer.

## License

Heron Broadcaster is free software, licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
