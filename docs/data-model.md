# Data model

The schema lives in `migrations/`; this page summarizes it. Tables not used yet are sketches and will change with their milestone. Database: MariaDB through `sqlx`, with migrations in `migrations/` that run automatically on startup.

## Library

| Table | Columns |
|---|---|
| `tracks` | id, path (relative to the library root, `/` separators, unique through a hash), file_hash (content hash: size + first and last 64 KiB), file_size, file_mtime_us, title, artist, album, musicbrainz_artist_id (the first of the tag), artists_key (hash of the artist and title tags the track's artists were linked from), duration_ms, enabled, missing_since, created_at, updated_at |
| `track_analysis` | track_id, analysis_version, params_hash (fingerprint of the analysis settings), integrated_lufs, true_peak_db, cue_in_ms, cue_out_ms, fade_out_start_ms, has_internal_silence, analyzed_at |
| `track_overrides` | track_id, cue_in_ms, cue_out_ms, crossfade settings, gap_killer_enabled (all nullable) |
| `analysis_jobs` | id, track_id, analysis_version, status (pending, running, done, failed), attempts, claimed_by, claimed_at, error, created_at, updated_at — queue between the scanner and `heron-analyzer` (see [Import and analysis](analysis.md#job-queue)) |
| `track_fingerprints` | track_id, chromaprint (compressed, first two minutes), created_at |
| `track_waveforms` | track_id, interval_ms (50), peaks (one byte per interval: the highest sample peak on a dB scale, 0 for −72 dBFS or below, 255 for full scale) — drawn by the web UI, computed with the analysis |
| `track_identifications` | track_id, status (identified, needs_review, kept: reviewed, the file keeps its own tags), reason, score, recording_mbid, release_mbid, candidates (JSON, for the review screen), updated_at |

## Track details and labels

See [M5b](roadmap/m5b-track-details-tags.md). The date a track was added is `tracks.created_at` (the scanner creates the track when it first finds the file).

| Table | Columns |
|---|---|
| `tracks` (added) | album_artist, genre, year, track_number, disc_number (tags); musicbrainz_recording_id, musicbrainz_release_id (tags); codec, bitrate_kbps, bitrate_mode (cbr, vbr, lossless), sample_rate, channels; details_version (what the scanner read: older tracks are read again) |
| `track_analysis` (added) | bpm (measured) |
| `track_musicbrainz` | track_id, release_id and recording_id (looked up for), album, record_label, catalog_number, isrc, first_release_year, genres (JSON), fetched_at |
| `track_programming` | track_id, bpm (set by hand, replaces the measured one), mood, sound_code (values of the scales in the `programming` settings), updated_at |
| `labels` | id, name (unique ignoring case), color |
| `track_labels` | track_id, label_id, created_at |

The era is not stored: it comes from the year of first release (`track_musicbrainz.first_release_year`, else `tracks.year`) and the eras of the `programming` settings.

## Library management

See [Library management](library.md).

| Table | Columns |
|---|---|
| `library_batches` | id, kind (import, organize, undo, review: a release picked on the review screen, edit: tags edited from the browser, delete, purge), undoes (the batch an undo reverts), created_at, finished_at |
| `library_moves` | id, batch_id, track_id, from_root (incoming, library, staging), from_path, to_path, method (rename, copy), temp_path, content_hash (copies), status (pending, moved, done, abandoned, undone), error — the journal, written before a file is touched |
| `track_markers` | track_id, intro_ms, intro2_ms, hook_in_ms, hook_out_ms, next_soft_ms, next_hard_ms (points imported from another software, shown on the track's page), updated_at |
| `track_imports` | id, created_by, created_at, summary (JSON: tracks, categories and labels created, tracks disabled), undone_at, undone_by |
| `track_import_changes` | import_id, track_id, previous (JSON: what the import changed, as it was, for undo) |
| `track_tag_overrides` | track_id, field (a tag: title, artist, album, album_artist, year, genre, track, disc), value (NULL removes the field), updated_by, updated_at. Tags kept in Heron instead of the files while `write_tags` is off; applied on top of the file's tags whenever the track's columns are read from its file |
| `track_tag_changes` | id, track_id, move_id, kind (tags: identification's, fields: edited from the browser), previous and applied tags (JSON), cover_embedded, restore (rebuilds the previous file byte for byte from the one written), created_at, undone_at |
| `library_tasks` | id, kind (organize_plan, organize_apply, undo, review_apply, artist_relations, edit_tags, musicbrainz_details, staging_import, delete, empty_trash), params (JSON, the request), status (pending, running, done, failed), result (JSON: the dry run's moves or the batch report), error, requested_by, created_at, started_at, finished_at — work the API asks of `heron-librarian` |

## Buffer zone and trash

See [Library management](library.md#the-buffer-zone).

| Table | Columns |
|---|---|
| `staged_groups` | id, source (upload, incoming), name, created_by, created_at — the files of one upload or one incoming folder, identified together |
| `staged_files` | id, group_id, staged_path (relative to the buffer folder), original_name, size, content_hash, status (receiving, identifying, ready, importing, imported, discarded, failed), tags (the file's own), duration_ms, has_cover, chromaprint, identification (JSON: status identified, needs_review or off, reason, score, MBIDs, candidates), choice (JSON: a release picked by hand, or own tags), base_tags (before hand edits), tag_edits (JSON: fields edited by hand), applied_tags (base_tags with tag_edits), planned_path, mood and sound_code (given to its track), flags (JSON), duplicate_of_track_id, duplicate_of_staged_id, track_id (once imported), error, uploaded_by, created_at, updated_at |
| `staged_file_categories` | staged_file_id, category_id, weight — categories its track is put in when imported |
| `staged_file_labels` | staged_file_id, label_id — labels its track gets when imported |
| `tracks` (added) | deleted_at (in the trash until purged; also marked missing) |
| `track_fingerprints` (added) | chromaprint_hash (to find the same audio) |

Library batches gain the kinds `delete` and `purge`, moves the origin `staging` (the trash is in the library, `.heron-trash/`), and library tasks the kinds `staging_import`, `delete` and `empty_trash` (restoring is an `undo`).

## Categories

| Table | Columns |
|---|---|
| `category_types` | id, name (unique): Song, Jingle, Ad from the start, and the station's own; no_two_in_a_row (rotation never plays two of its tracks in a row; Jingle's is on), sends_metadata (its tracks' artist and title go to the stream servers; off for Jingle and Ad) |
| `categories` | id, name, color, type_id (none: no type), fx_player (the FX player's picker opens on it), separation (JSON: the windows it overrides, `NULL` for none) |
| `track_categories` | track_id, category_id, weight |

## Artists

See [Rotation – Artists with several names](rotation.md#artists-with-several-names).

| Table | Columns |
|---|---|
| `artists` | id, name (shown), musicbrainz_id (unique) |
| `artist_names` | id, artist_id, name (as first seen), normalized (lowercase, no accents, spaces collapsed; unique through a hash), by_hand (moved to its artist by hand) |
| `artist_similar` | artist_id, similar_id (one row per pair, the smaller ID first) — separation treats them as one artist |
| `artist_suggestions` | id, artist_id, other_id (the smaller ID first), source (names, musicbrainz), reason, status (pending, accepted, dismissed) |
| `track_artists` | track_id, artist_id, position — linked from the tags by `heron-api` |

The artist groups sketched before M6 became similar links.

## Restrictions

See [Rotation – Restrictions](rotation.md#restrictions).

| Table | Columns |
|---|---|
| `restrictions` | id, target (track, artist, category), target_id, rule (JSON, tagged by `kind`: `daypart` with mode, days, start_hour, end_hour; `date_range` with mode, start, end, every_year; `not_after` with category_id), enabled, note, created_at, updated_at |

Targets are not foreign keys: restrictions on a deleted category go with it, merged artists hand theirs over, others are ignored.

## Clockwheels

| Table | Columns |
|---|---|
| `clocks` | id, name, looping (carries on across hours instead of starting over at each hour) |
| `clock_slots` | clock_id, position, category_id, selection_method (least_recently_played, random, weighted), enforce_rules, separation (JSON: the windows it overrides) |
| `clock_schedule` | day_of_week (ISO: 1 = Monday), hour (station time), clock_id |
| `rotation_picks` | id, track_id, planned_at, clock_id, slot, hour_start, category_id, step, explanation (JSON, see [Rotation](rotation.md#rotation-model)), engine_started_at and entry_id (the engine's queue entry), play_id — the selection log |

## History

| Table | Columns |
|---|---|
| `play_history` | id, track_id (NULL for files outside the library and deleted tracks), artist, title, started_at, ended_at, played_ms, end_reason (finished, skipped, stopped, replaced; NULL while playing or when the API missed the end), deck (a, b, fx), source (rotation, request, fallback, manual), relaxation_applied (JSON: pick_id, step, description, for relaxed rotation picks) — recorded by `heron-api` from the engine's track changes (see [API and control plane](api.md#play-history)) |

## Settings

| Table | Columns |
|---|---|
| `settings` | name, JSON value. `analysis`: Gap Killer and crossfade point detection ([Import and analysis](analysis.md#settings)); `library`: AcoustID key, layout, covers ([Library management](library.md#settings)); `playout`: crossfade and loudness ([Audio engine](audio-engine.md#playout-settings)); `queue`: autofill and how many tracks wait ([API](api.md#queue-manager)); `engine`: whether the API manages the engine's configuration yet ([API](api.md#engine-configuration)); `rotation`: separation windows, fallback category, weight factors; `station`: time zone ([Rotation](rotation.md#scheduling)); `programming`: what counts as a play, eras, mood and sound code scales ([API](api.md#track-details)); `statistics`: minimum session length, retention ([Listener statistics](statistics.md#settings)) |

## Encoders and outputs

See [Encoders and outputs](outputs.md). The engine never reads these tables: the API pushes them over gRPC.

| Table | Columns |
|---|---|
| `encoder_profiles` | id, name, encoder (mp3, aac, opus, vorbis, external_ffmpeg), bitrate_kbps, sample_rate, channels, options (JSON: VBR/CBR, ffmpeg binary path and arguments, …). A fresh install has `MP3 128k` |
| `outputs` | id, name, kind (icecast, shoutcast_v1, shoutcast_v2, hls, file, pipe), encoder_profile_id, config (JSON: host, port, mount, credentials, HLS directory and segment settings, …), metadata_enabled, enabled |

## Listener statistics

See [Listener statistics](statistics.md). No IP address is ever stored.

| Table | Columns |
|---|---|
| `outputs` (added) | statistics (JSON: the stream server's `admin_username`, `admin_password`; never pushed to the engine) |
| `listener_samples` | output_id (deleted with the output), sampled_at (the minute), listeners. Kept `retention_days` |
| `listener_sessions` | id, output_id (deleted with the output), listener_key (BLAKE3 of address and player, keyed with the day's key), user_agent, client_id (the server's ID for the connection), day (station time), started_at, last_seen_at, ended_at (NULL while connected), split (ended at midnight, still connected), duration_s. Kept `retention_days` |
| `listener_day_keys` | day, key (random, 32 bytes). Deleted once its day can get no more sessions |
| `listener_totals` | output_id (NULL: the whole station; deleted with the output), day, hour (NULL: the whole day), minutes (measured), peak, average, listening_hours, sessions, session_s (their total length), distinct_listeners (NULL without sessions). Kept |

Statistics settings are in `settings` (`statistics`: `min_session_s`, `retention_days`).

## Integrations and access

| Table | Columns |
|---|---|
| `webhooks` | id, name (unique), url, enabled, events (JSON: `track.started`, `track.ended`, `track.skipped`), include_fx, secret (in clear: it signs the deliveries; never sent back), content_type, body_template (NULL: the default JSON body), created_at, updated_at |
| `webhook_deliveries` | id, webhook_id (deleted with the webhook), event (or `ping`), body (as sent), status (pending, delivered, failed), attempts, next_attempt_at (while pending), last_attempt_at, last_status_code, last_error, last_response (first 1 KiB), created_at, delivered_at. Kept 30 days |
| `users` | id, username, password_hash (Argon2id), role (viewer, operator, admin), enabled, theme (system, light, dark: the web UI's), language (system or a language tag: the web UI's), full_width (the web UI uses the whole screen), last_login_at, created_at, updated_at |
| `sessions` | token_hash (BLAKE3 of the cookie's token), user_id, created_at, last_seen_at, expires_at (30 days after the last use) |
| `api_tokens` | id, user_id, name, token_hash, created_at, last_used_at, expires_at, revoked_at |

Authentication is local only for now ([Decisions](decisions.md#decisions-made-after-the-handoff), [API](api.md#authentication)).

## Relationships

```
tracks 1──1 track_analysis
tracks 1──0..1 track_overrides
tracks 1──* analysis_jobs
tracks 1──0..1 track_fingerprints, track_identifications, track_waveforms, track_musicbrainz, track_programming
tracks *──* labels            (through track_labels)
library_batches 1──* library_moves 1──0..1 track_tag_changes *──1 tracks
staged_groups 1──* staged_files *──0..1 tracks
staged_files *──* categories, labels   (through staged_file_categories, staged_file_labels: given to its track)
tracks *──* categories        (through track_categories, with weight)
tracks *──* artists           (through track_artists)
artists 1──* artist_names
artists *──* artists          (through artist_similar, artist_suggestions)
restrictions *──1 tracks | artists | categories
clocks 1──* clock_slots *──1 categories
clock_schedule *──1 clocks
rotation_picks *──1 tracks, 0..1 play_history
outputs 1──* listener_samples, listener_sessions, listener_totals
play_history *──1 tracks
webhooks 1──* webhook_deliveries
users 1──* sessions, api_tokens
encoder_profiles 1──* outputs
```
