# Changelog

What changed in each release of Heron. Before upgrading, read the release's notes in [Upgrading](docs/upgrading.md#release-notes).

## Edge

What `main` has since the last release: it is published as the `edge` images. Read the [Edge upgrade notes](docs/upgrading.md#edge) first.

### Added

- **AGC and multiband processing** (M10): a processing chain on each station's master bus: a gated AGC that brings the level to the station's loudness target, a stereo expander, a bass EQ, 5-band and dual-band processors (compressor, expander or limiter per band) and a final look-ahead limiter or clipper, all without clicks. A **Processing** page shows the chain with its meters, and presets (Gentle, Pop, Dense, Rock, Talk) are shared by the stations ([Audio engine – Master AGC chain](docs/audio-engine.md#master-agc-chain)).
- **Backups** (M12): every day, the configuration (stations, programming, outputs, users, what is set on each track, medleys) and a dump of the database, in a folder that can be a network share. A restore from the web that fails half-way changes nothing, and a banner says how to fix a backup folder that cannot be written ([Backups](docs/backups.md)).
- **Encrypted and tested backups** (M15): optional encryption with [age](https://age-encryption.org/v1) (Heron keeps only the public key, and a restore asks for the key), a checksum file for each backup, every backup read back as it is written, key checks for each admin so that a lost key is noticed before a restore needs it, a *Test* button, and a banner when no test has passed for 30 days ([Backups – Encryption](docs/backups.md#encryption)).
- **Status bar** (M16) at the bottom of every page: the two decks with their track and time, the station's listeners and, for operators and admins, CPU, memory and load.
- **About page** (M17), linked at the bottom of the menu: the version, commit and build date of the web interface and of each service and whether it answers, and for admins the machine (system, kernel, architecture, container, time zone, clock, CPU, memory), the database and FFmpeg versions and the active configuration (a secret only shows whether it is set), with a button that copies it all for a bug report. The engine answers a new `GetAbout` call; the analyzer and the librarian write a heartbeat to the database every 30 seconds. The images carry their build date (`HERON_BUILD_DATE`) next to the commit.
- **AAC-LC encoder** (first part of M11): FFmpeg's native `aac` encoder, as an ADTS stream for Icecast, with the default `AAC 96k` profile and an *AAC-LC* choice in the Outputs page. It is built with the optional `aac` Cargo feature, which links the system's libavcodec; Heron's engine image has it. See [Encoders and outputs](docs/outputs.md#built-in-aac).
- **Track levels:** a track's level in dBFS on its waveform, with its lowest and highest.
- **History** marks where the clocks change and which pass of the repeated hour a time belongs to.
- **Spanish and German** web interface, next to English and French.
- **Rotation:** hours without a clock rotate the station's library.

### Changed

- **Clearer page names**, in all four languages: *Buffer* is now **Import music**, *Import* is **Migrate**, *Review & organize* is **To organize**, *Selection log* is **Rotation log** and *AGC* is **Processing**. Their addresses changed with them (`/import-music`, `/migrate`, `/to-organize`, `/rotation-log`, `/processing`) and the old ones are not redirected: update any bookmark. The API is unchanged.
- **Several stations** (M13) is finished: two stations never share an engine, a station's schedules are under its routes, and `scripts/upgrade.sh` hands over each station's engine, so a second station no longer restarts cold.
- **Dependencies** are all up to date. The Docker Compose files now use MariaDB 12.3 (was 11.8), Icecast 2.5.0 (2.4.4) and ffmpeg 9.0.2 (7.1.1), and the web image Node 26 and nginx 1.31.
- **The API answers requests it cannot read** as JSON errors.
- **The web interface** says when it and the API are not from the same version, and suggests reloading the page first.

### Fixed

- A track resumed by a restarted engine, or by one taking over during an upgrade, is no longer reported as a new play: no second `track.started` webhook and no second row in the history.
- A hot handover retried with a longer lead waits for its stop, instead of giving up after 2 s with the track still on air. An engine that handed over no longer removes its successor's socket.
- `scripts/upgrade.sh --swarm` updates each service by the digest it pulled, so a moving tag like `edge` is upgraded for the API, web, analyzer and librarian too (only the engines were).
- The library scanner and the incoming folder no longer scan again after every scan on Linux: reading files is not a change.
- Writing the tags of an MP3 with a Picard `WORK` frame no longer fails.
- Undoing a tag edit and restoring from the trash work with MariaDB 12.3.
- A restore no longer fails while webhook deliveries are retried.

## 0.2.0 – 2026-11-01

Milestones M9 (deck EQ), M13 (several stations, see [Not yet](#not-yet)) and M14 (several schedules), medleys, category types, imports from another software and the external FFmpeg encoder. Upgrading from 0.1.0 keeps the station on air: see [0.1.0 to 0.2.0](docs/upgrading.md#010-to-020).

### Added

- **Several stations** in one Heron, sharing the library ([Several stations](docs/stations.md)). Each station has its own engine (one more service of the deployment, `HERON_ENGINE_URL_<CODE>`), programming, queue, playout settings, EQ, outputs, history and webhooks. Categories, medleys and restrictions are tagged with the stations they belong to; artist, title and track windows can apply across stations; plays are counted per station; users get a role on each station. A station switcher appears in the web interface once there are two. The single-station API routes and public endpoints answer for the first station.
- **Several schedules per station:** a default schedule, and others that play during their periods (a season, the summer, every year). The schedule editor paints a whole day, an hour of every day, the week or a rectangle, and copies a day.
- **Deck EQ:** a 10-band EQ for the music decks and another for the FX player, ±12 dB, heard as the sliders move, without clicks, with presets for several styles of music ([Audio engine – Deck EQ](docs/audio-engine.md#deck-eq)). Flat and bypassed until it is turned on.
- **Medleys:** the hooks of several songs strung together with the station's liners, between them or over the music, built from a recipe, edited on a timeline (moves, nudged points, crossfades, fade-in and fade-out, where the music goes down and back up around a liner), previewed, then mixed into a track of the library named after its artists or titles and the day ([Medleys](docs/medleys.md)).
- **Category types:** Song, Jingle, Ad and the station's own. A type can never play twice in a row, and can send no metadata to the stream servers (the last song's stays). A jingle can play on the FX player over the next song's intro, ending where its vocals start.
- **Importing from another software:** an export of another radio automation software (titles, artists, categories, fades, intro and hook points) matched to the library by file name, checked in a dry run where conflicts are settled, applied as one import that can be undone ([Importing from another software](docs/import.md)).
- **External FFmpeg encoder:** any codec of the station's own `ffmpeg`, such as AAC+ with `libfdk_aac`, configured from the Outputs page (`HERON_FFMPEG`, [Encoders and outputs](docs/outputs.md#external-ffmpeg-encoder)).
- **`queue.changed` webhook** and a public queue endpoint: the upcoming tracks in airing order, for a website or a display.
- **Tags kept in Heron** when `write_tags` is off: edits are saved in the database and applied over the file's tags; a track can go back to its file's tags.
- **Where the next track starts** is always shown on the track page and set like a cue point, also on a short track; a transition can be tried and listened to from a track.
- **Web interface:** the category of each deck and in *Up next*, a breadcrumb above the page title, a per-user option to use the whole width of the screen, range selection with Shift + click and Ctrl/⌘ + click, enable or disable a selection, a category created from a track, a selection or the buffer, the intro countdown and the playhead time on the decks, imported intro and hook points on the track page, enforce rules turned off from the slots running short.
- **Forks** publish their own images: `HERON_REGISTRY` in `.env` picks where a station pulls them from.

### Fixed

- Imports up to 64 MiB pass through the web container's nginx.
- A track's year shows without a thousands separator.
- Requests sent to a station keep their body, and the web interface opens one live socket per station.

### Not yet

- Several stations (M13) works and is documented, but two of its acceptance criteria are still to be demonstrated: two stations on air at once with a hot handover of one leaving the other untouched, and a medley tagged with two stations landing in both stations' medley categories. `scripts/upgrade.sh` hands over the first station's engine only: the other stations' engines restart cold, covered by Icecast's fallback mount.
- AGC and multiband processing (M10), more encoders, sinks and live DJ (M11), configuration backups (M12).

## 0.1.0 – 2026-10-26

The first release, milestones M0 to M8: library import and analysis, the buffer zone for new music, library management, playout with two decks, crossfades, Gap Killer and loudness normalization to Icecast, 24/7 reliability, upgrades on air by hot handover, rotation with categories, clocks and a weekly schedule, webhooks and public endpoints, listener statistics, and the web interface in English and French.
