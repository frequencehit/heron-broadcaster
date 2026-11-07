# Changelog

What changed in each release of Heron. Before upgrading, read the release's notes in [Upgrading](docs/upgrading.md#release-notes).

## Edge

What `main` has since the last release: it is published as the `edge` images. Read the [Edge upgrade notes](docs/upgrading.md#edge) first.

### Added

- **Play ratio** per track: an exact quota of plays, from 0.1 to 1 (2 times out of 10 at 0.2), with a counter per station, shown on the track's page and in the rotation log. 0 disables the track; re-enabling it asks for its ratio. A column, a filter and a bulk edit in the track list. A **station can have its own ratio** for a track, the track's ratio being the default; at 0 the track is disabled on that station only. Set on the track's page (one line per station), in the track list for the station shown, or imported from a CSV file with a column per station. Who disabled a track and when is shown on its page and in the list, and a station's counters can all be reset to 10 at once ([Rotation – Play ratio](docs/rotation.md#play-ratio)).
- **Daypart separation:** a track played in a daypart (4 of 6 hours or 6 of 4 hours, in the station's time zone) does not play in the dayparts that follow it, across midnight too. Off by default and set per category (the station sets its division of the day), relaxed between album and artist separation, and shown in the rotation log ([Rotation – Daypart separation](docs/rotation.md#daypart-separation)).
- **Same daypart, the following days:** a category can also keep a track out of the daypart it played in on the next 1 to 7 days (played in B on Monday, not in B on Tuesday), with or without daypart separation. Relaxed with daypart separation and counted in the rotation log ([Rotation – Daypart separation](docs/rotation.md#daypart-separation)).
- **Daypart separation per track:** a track can have its own daypart separation (0 to 10) and its own days in the same daypart (0 to 7), each replacing its category's in every category it is in, on every station (cut to each station's maximum). Relaxed like the category's; the rotation log says which applied. On the track's page, and in the track list with a column, a filter and a bulk edit; kept in configuration backups ([Rotation – Daypart separation](docs/rotation.md#daypart-separation)).
- **Linked clock slots:** a slot *linked to the next slot* (a *jingle 90s* before a 90s track) plays only with the next slot's track: at the end of the hour, or when the next slot finds nothing, the jingle is skipped too, and the rotation log says why. In the queue the jingle is joined to its track: removing the track removes it, moving the track moves it, and nothing goes between them ([Rotation – Linked slots](docs/rotation.md#linked-slots)).
- **When a track played**, on its page: its plays of the last 30 days week by week, and the hours of the week it played in over 30, 90 or 365 days, for one station or all of them, all plays or rotation only. Hovering a day or an hour gives the plays; a click on a day opens the history at that day, for that track ([Web UI – Plays by day and hour](docs/web.md#plays-by-day-and-hour)).
- **Cover art:** each track's cover, kept in Heron (600 and 96 px, each image once, as files on a `covers` volume, in backups when set by hand), per album with exceptions per track. Found embedded in the files, beside them (`cover.jpg`, `folder.jpg`, `front.jpg`) or on the Cover Art Archive (by `heron-librarian`, one request a second; unsure matches wait on *To organize* as candidates), or set by hand from the track's page (an image or its address). Shown in the track list (a column, a *No cover* filter, *Find covers* on a selection), on the track's page, On air, the queue and the history; `cover_url` in the API, the public endpoints and webhooks (`{{cover_url}}`), where the station's logo stands in for a missing cover ([Library management – Cover art](docs/library.md#cover-art)).
- **Modules:** medleys, cover art, listener statistics and webhooks can be turned off for the whole installation (*Administration › Modules*) or for one station (*Settings › Station › Modules*). A module turned off is hidden (menu, pages, settings, widgets), answers `404` on its routes, does nothing in the background (no medley on air, no cover lookup, no stream server read, no webhook sent) and keeps its data for when it is turned on again. Everything stays on after the upgrade ([Modules](docs/modules.md)).
- **More modules:** restrictions, the equalizer and processing can be turned off for the installation or a station; off, they are hidden and not applied (rotation ignores the restrictions, the EQ and the AGC chain are bypassed on air without a click), and their settings are kept. Trash, To organize, Artists and Labels can be turned off for the installation: hidden, their routes answer `404`, their data kept. Everything stays on after the upgrade ([Modules](docs/modules.md)).
- **Station filter** on the Tracks, Categories and Medleys pages: the switcher's station at first, another station or all of them, kept in the browser per page. On a station, the track list shows what rotation can play there: the tracks of its categories, less those it keeps out of its library. A *No category* choice in the track list's categories, and `on_station_id` and `no_category` on `GET /tracks` ([Web UI – Station filter](docs/web.md#station-filter)).
- **Two-factor authentication:** an authenticator app (TOTP) and passkeys or security keys (WebAuthn), several per user, asked after the password at sign-in; 10 recovery codes, each used once; an admin can require it per role, and a user who must have it sets it up before anything else. `heron-api user reset-2fa <name>` removes a user's second factors. API tokens are not concerned ([API – Two-factor authentication](docs/api.md#two-factor-authentication)).
- **Stream servers** per station (*Outputs › Servers*): address, port, source and admin credentials and public address typed once, picked by every output going to that server, which keeps only its mount, encoder profile and the station details it overrides. Changing a server's password reconnects its outputs; a server in use cannot be deleted. **Relay servers** repeat a master's mounts: Heron reads them for the listener statistics, adds their listeners to each output's (leaving out the relay's own connection to the master) and shows the listeners per server ([Encoders and outputs – Stream servers](docs/outputs.md#stream-servers), [Listener statistics – Relay servers](docs/statistics.md#relay-servers)).
- **Station details** (name, description, genre, website, public) sent to the stream servers are set once for the station (*Settings › Station*), and each output can override any of them ([Encoders and outputs – Station details](docs/outputs.md#station-details)).
- **Duplicate** an output: a disabled copy with the same server, profile and details, its mount suffixed `-copy`.
- **About: Clear the stopped ones** (admins) takes the workers that stopped reporting off the list, such as the containers of earlier runs (`DELETE /about/stopped`).

- **Draw depth:** a slot draws at random (or by weight in a weighted slot) among the 20 % of its category's tracks played longest ago on the station, at least 3 tracks, never played ones included, so two rounds of a category do not play in the same order. When nothing in the draw passes the rules, it widens by 10 points up to 50 % before any rule is relaxed; the rules are then relaxed with the draw at its maximum, and the most recently played half is never drawn from. Set per station, per category and per clock slot (start and maximum), shown in the rotation log and checked by simulations ([Rotation – Draw depth](docs/rotation.md#draw-depth)).
- **Exact times:** a clock slot can start at a time of the hour (`:00:00`, `:30:00`). **Hard:** what plays fades out over 1.5 s and the slot starts, to the second. **Soft:** rotation plans the tracks before it to end in a window (10 s before to 30 s after, per slot); a track that would end after the window is faded out at its end; a gap before the window is filled by a station medley, shortened from its end when too long (the last liner + hook pairs left out, the closing liner kept, without a new render), or by a track of the station's new filler category, else the slot starts early. The engine cuts at the exact time on the server's clock, across restarts and handovers. The rotation log says how each exact time was met, simulations show it and their audit flags missed windows ([Rotation – Exact times](docs/rotation.md#exact-times)).
- **To do** (formerly *To organize*, now at `/to-do`): tabs with their count for the songs with no intro, no hook or no cover (enabled songs whose file is there, in a category of the Song type), each linking to the track's page, next to the identifications to review, the covers to check, organizing and the changes. `GET /library/todo` counts them, and `GET /tracks` has `filter=no_intro`, `filter=no_hook` and `active_songs` ([Web – To do](docs/web.md#to-do)).
- **Looking ahead before exact times:** within 15 minutes or 4 slots of an exact time, rotation simulates 8 endings up to it with the real rules and queues the first pick of the best one: fading nothing out first (a filler jingle rather than a song faded out), then ending in the window closest to the exact time, with the least filling, with the fewest rules relaxed. Before a hard exact time, a pick widens its draw before choosing a track that would be cut. The rotation log shows the endings simulated and how the best one ends ([Rotation – Exact times](docs/rotation.md#exact-times)).
- **The whole queue on On air** (Account › Display, off by default): the *Up next* card lists every waiting track, to reorder, play next and remove there, without the *Edit the queue* button; adding tracks stays on the Queue page. `on_air_whole_queue` in `GET /auth/me` and `PUT /auth/preferences`, kept in configuration backups (off in older ones) ([Web UI – Themes](docs/web.md#themes)).

### Changed

- **Planned starts count the crossfades:** rotation, the queue manager and the queue's expected starts start each track where the one before hands over to it (its mix point, as the engine plans the transition), instead of at its end. The engine's state gives it for each waiting entry (`next_start_ms`) ([Rotation – Exact times](docs/rotation.md#exact-times)).
- **Least recently played slots draw among the oldest tracks** instead of always taking the oldest one, and so do hours without a clock: see *Draw depth* above. Random slots are unchanged.
- **A track's page no longer warns** that its tags are kept in Heron rather than in the file: *Back to the file's tags* sits next to *Edit tags* instead, while there is something to go back from.
- **Relaxation steps** are numbered 0 to 12: daypart separation comes in at steps 3 and 4, so artist, title, track and the fallback category moved two steps. The picks already logged were renumbered.
- **A disabled track does not play at all** any more: it cannot be queued by hand, and the queue manager takes it out of the queue. Before, it was only left out of rotation.
- **Cover images are files** on a new `covers` volume (`HERON_COVERS_DIR`) shared by `heron-api` and `heron-librarian`, not in the database: the database keeps what is known of each image (hash, size), and its dump no longer holds images. The API moves the images of an earlier edge build to the volume when it starts, once ([Upgrading – Edge](docs/upgrading.md#edge)). The configuration file still holds the covers set by hand and the station logos with their images, and older files restore as before.
- **Listener statistics read each stream server once a minute**, for all its mounts, instead of once per output.
- **Outputs in the API** give a `server_id` and only their mount in `config`; their `statistics` (the admin credentials) moved to the stream server. A migration groups the existing outputs into servers ([Upgrading – Edge](docs/upgrading.md#edge)).
- **Settings** are in tabs: *Station*, *Programming*, *Audio*, *Library* and *Statistics*.
- **Page width** (Account › Display): *Normal*, *80 %* or *100 %* of the window, instead of a *whole width* checkbox. The API's `full_width` is now `width` (`normal`, `wide` or `full`); a migration and older configuration backups turn `full_width` into `full`.
- **The track list's bar**: search on its own line, then the station, category, play ratio and sort menus, each with its name above it; the status menu (playable, missing, silent…) and the daypart separation filter moved to *More filters*, which opens by itself when one of them is set (as from the library's *missing* or *silent* links).
- **Settings › Programming › Rotation** is split into titled sections (separation, dayparts, draw depth, when no track fits, weight after a play, the station's library, across stations), each with a one-line explanation; how each slot method draws is folded away, the draw depth has a diagram that follows its two values, and the two divisions of the day sit on one line.
- **A track's page** is in tabs: *Sound*, *Programming*, *Details* and *Plays*.
- **Clocks save themselves** a moment after each change: no *Save* button to forget. The page says *Saving…*, *Saved* or why a change was not saved; a new clock is still created with its button.
- **To organize is now To do**, in all four languages, at `/to-do`: the old address leads there. Its *Identifications* tab is *To identify*, and the covers to check are at the top of *Missing covers*.
- **A track's page:** the playhead's time and level sit under the waveform, on the right; *Try a transition* draws its graph at the waveform's height whatever the page's width.

### Fixed

- The shared routes that take a station to look at (`GET /tracks?station_id=`, `GET /categories?station_id=`, `GET /medleys?station_id=`, `GET /tracks/{id}/plays/grid?station_id=`, and a selection's filters) answer `404` for a station the user does not see, as that station's routes do.
- Backups have their database dump again after MariaDB moves to a new version: the Compose file has MariaDB upgrade its own tables when it starts (`MARIADB_AUTO_UPGRADE`). `mariadb-dump` failed on them before (*Column count of mysql.proc is wrong*).
- The queue starts a track with an [exact time](docs/rotation.md#exact-times) at that time at the latest, on a colored background, instead of an estimate counted from the tracks before it, and the tracks after it follow from that time.
- **Autofill stopped after an exact time** until the hour changed, when the exact slot was the clock's first slot: rotation went back to it, found it due again and, unable to play it twice, picked nothing. A clock that does not loop now goes on with the slots after the one just played, and the slot just played is never due again in the same hour. The log also says rotation found nothing to queue instead of blaming restrictions.
- A user with the whole width in an older backup (`full_width`, written as `1`) gets the *100 %* width back on restore, not the normal one.

## 0.3.0 – 2026-11-05

Milestones M10 (AGC and multiband processing), M12 (backups), M13 (several stations, finished), M15 (encrypted and tested backups), M16 (status bar) and M17 (About page), a first part of M11 (the AAC-LC encoder), Spanish and German, and up-to-date dependencies. Upgrading from 0.2.0 keeps the station on air: see [0.2.0 to 0.3.0](docs/upgrading.md#020-to-030).

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
- **A standby prepared twice in a row** (a handover retried after a late `Ready`) no longer loses its cued track, nor answers `Ready` for a decoder that is not open yet: each cue carries its own generation ([#45](https://github.com/dehy/heron-broadcaster/issues/45)).

### Not yet

- The rest of M11: Opus and Vorbis, HLS, Shoutcast, file recording, an external process pipe and live DJ input. HE-AAC works through the external FFmpeg encoder only.

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
