# Locked decisions

These decisions are fixed. If one looks wrong, stop and ask instead of working around it.

| Area | Decision |
|---|---|
| Language | **Rust** for the engine, analysis, rotation and API. |
| Database | **MariaDB**, accessed with `sqlx` (MySQL driver). Migrations live in the repository and run automatically on startup. |
| Frontend | **React + Vite + TypeScript** (a web app, not React Native). |
| Deployment | **Docker Compose**, one service per process. *(Docker Swarm on a single node is supported too, see [Upgrading](upgrading.md).)* |
| Analysis | Loudness, cue points, silence trimming and crossfade points are computed **when a track is imported** and stored in the database. Never during playback. |
| Clockwheels | Clockwheels are **data**: ordered slots stored in the database and edited in the UI. No scripting language for now. |
| Audio path | **The engine never touches the database in the audio path.** |
| Roadmap order | Audio processing (AGC, multiband) **comes last** in the roadmap. |
| Outputs | **Bring your own streamer:** outputs are pluggable sinks behind a trait. Icecast first. |
| Codecs | **Heron never ships or links fdk-aac** (license incompatible with AGPL). LAME (LGPL), libopus and libvorbis (BSD) are fine. AAC-LC is built in using FFmpeg's native `aac` encoder (LGPL). fdk-aac / HE-AAC only through a user-provided `ffmpeg` binary. *(Revised after the handoff, see below.)* |

## Decisions made after the handoff

| Area | Decision |
|---|---|
| Authentication | **Local users only for now** (users and sessions or API tokens in MariaDB). OIDC may be added later. |
| Analysis worker | The analysis worker is **its own modular service**, `heron-analyzer`, not part of `heron-api`. See [Architecture](architecture.md). |
| Multiple outputs | A station can stream to **several stream servers at once**, each output with its own encoder profile. See [Encoders and outputs](outputs.md). |
| Encoder selection | The user **selects the encoder** per profile: MP3, AAC-LC, Opus, Vorbis (built in) or external FFmpeg. This replaces the generic "pipe to ffmpeg" approach for encoding. |
| AAC | **Built-in AAC-LC without HE is sufficient.** fdk-aac (HE-AAC) is only available when the user provides their own fdk-aac-enabled `ffmpeg` binary; Heron does not ship it. |
| HLS | Heron has an **HLS output** (segments + playlist). |
| Rotation relaxation | Album → artist → title → track separation, each halved before being dropped (track only reduced to a quarter, never dropped), then the fallback category. See [Rotation](rotation.md#rule-relaxation). |
| Default encoder profiles | `MP3 128k` (CBR, 44.1 kHz, stereo) and `AAC 96k` (AAC-LC, 48 kHz, stereo). Opus is optional. See [Encoders and outputs](outputs.md#default-profiles). |
| UI theme | The web UI has light and dark themes and **follows the system setting** by default. |
| Platforms | Heron must **run on a Raspberry Pi**. Every image is built for **amd64 and arm64**, and CI tests both natively. Defaults leave CPU headroom for the engine (the analyzer uses CPUs − 1 workers). |
| Library files | Heron **manages the library**: files dropped into an incoming folder are identified, tagged and filed under Artist / Album / Track. Moves are journaled and can be undone; files are never overwritten. *(Revised for M5c: new files, uploaded or dropped into the incoming folder, wait in a buffer zone until someone imports them; files are deleted only on request, through a trash they can be restored from until it is purged. See [M5c](roadmap/m5c-track-management.md).)* An existing library is reorganized only on request, after a dry run. Only `heron-librarian` mounts the library read-write; every other service mounts it read-only. Track paths are stored relative to the library root. See [Library management](library.md). |
| Track identification | AcoustID fingerprint lookup, then MusicBrainz. Matches are **applied automatically when confident**, otherwise they wait for review in the web UI. Each station uses **its own AcoustID key**. See [Library management](library.md#identification). |
| Cover art and layout | Cover art is saved as `cover.jpg` in the album folder (embedding is optional, off by default). Default layout `{album_artist}/{album}/{disc-}{track:02} {title}.{ext}`. |
| Librarian | Files are moved and tags written by **`heron-librarian`, its own service**, so the API keeps a read-only mount. Stations that want their files left alone turn the library setting `write_tags` off: tags edited from the browser are then kept in Heron's database (`track_tag_overrides`), applied on top of the files' tags. See [Library management](library.md#keeping-tags-in-heron-instead-of-the-files). |
| Uploads | `heron-api` writes uploads **only into the buffer folder**, never into the library; `heron-librarian` imports them from there. See [M5c](roadmap/m5c-track-management.md). |
| Silence detection | Gap Killer silence must be below the relative thresholds **and** below an absolute ceiling of −50 dBFS, so soft intros are kept. See [Import and analysis](analysis.md#what-counts-as-silence). |
| Artist names | Artist tags are split into several artists **on "featuring" only**; names are matched ignoring case, accents and spaces; anything else is linked by hand. Artists can be marked **similar** (separation treats them as one), replacing artist groups. See [Rotation](rotation.md#artists-with-several-names). |
| Restrictions | M6 ships **dayparting, date ranges and category sequences**, on tracks, artists and categories. Quotas ("no two tracks over 8 min in an hour") may come later. |
| Station time | Clocks and restrictions follow the **station's time zone**, a setting (IANA name) defaulting to the server's. |
| Backup encryption | Backups are encrypted with **age** (X25519), optional. **Heron keeps only the public key**: the secret key is shown once, never stored, and asked for on restores. No passphrases. See [Backups – Encryption](backups.md#encryption). |
| Monitoring | Heron **does not bundle Prometheus or Grafana**. Services push their metrics with Prometheus remote write (or are scraped); Heron ships a Grafana dashboard (JSON) and Prometheus alert rules to import into the station's own Grafana or Grafana Cloud. Monitoring and error reporting must never stop a service. See [Monitoring](monitoring.md). |
| Engine upgrades | An engine is upgraded by a **hot handover on the same host**: the new engine starts as a standby, and the old one passes it its state and its open output connections (file descriptors over a unix socket in the state volume) before exiting. The TCP connections to stream servers are held by `heron-relay`, a process that only carries bytes, since a TCP socket does not outlive its container's network. Rejected: a streaming service doing the encoding (its own upgrades would drop the listeners, and it would change often), sharing a network namespace between engines (not in Swarm), and swapping between two Icecast mounts with `fallback-override` (Icecast only, and the listeners move between mounts). An Icecast fallback mount covers the restarts that are not handovers. See [Engine handover](handover.md). |
| Several stations | One Heron can run **several stations**: the library, artists, categories, medleys and users are shared; programming, playout, history and outputs are per station, with **one engine per station**. Categories and medleys are **tagged with the stations** they are used on (not a prefix in their name); roles are given per station; separation can look across stations. See [Several stations](stations.md). |
