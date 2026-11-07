# API and control plane

`heron-api` is the station's control plane: the REST API and live WebSocket the [web UI](web.md) uses, the queue manager feeding the engine, the play history, and the configuration it pushes to the engine. Built in [M4](roadmap/m4-api-control-plane.md), completed for the web UI in [M5](roadmap/m5-web-ui.md); rotation came with [M6](roadmap/m6-rotation-clockwheels.md), webhooks and the public endpoints with [M7](roadmap/m7-webhooks-output-api.md).

The engine never depends on it: when the API is down, the engine plays what it has queued with the configuration it last received ([Audio engine](audio-engine.md)).

## Basics

- Every route is under `/api/v1`. `heron-web` proxies `/api` to `heron-api`, so the API is at `http://<host>:8000/api/v1`.
- Requests and responses are JSON. Times are Unix time in milliseconds (`*_unix_ms`), durations in milliseconds (`*_ms`).
- Errors are `{"error": "<code>", "message": "…"}`: `invalid_request` (400), `unauthorized` (401), `forbidden` (403), `two_factor_required` (403: set up a second factor first), `not_found` (404), `module_off` (404: the route's [module](modules.md) is turned off), `conflict` (409), `invalid` (422), `too_many_requests` (429), `engine_unavailable` (503), `internal` (500). Requests the API cannot read are answered the same way, with a message that says what is wrong: a body that is not JSON or not of the expected shape (`invalid` for a field of the wrong type or an unknown value, `invalid_request` otherwise), a path or query parameter of the wrong type, an unknown route (`not_found`) or method (`invalid_request`).
- **OpenAPI:** the document is generated from the code and served at `/api/v1/openapi.json`; `heron-api openapi` prints it. It is the reference for every route and type below.

## Authentication

Local users only for now ([Decisions](decisions.md#decisions-made-after-the-handoff)).

| Role | Can |
|---|---|
| `viewer` | See everything except users and secrets (output passwords, the AcoustID key are never sent back to anyone) |
| `operator` | Also run the station: queue, skip, pause, FX, the deck EQ, the AGC and their presets, track overrides, intro and hook points and categories, queue and rotation settings, clocks, the schedule, restrictions, artists, simulations, the review of identifications |
| `admin` | Also playout, analysis, library and station (time zone) settings, organizing the library and undoing batches, encoder profiles, outputs, webhooks and users |

With [several stations](stations.md#roles), these roles are given per station, plus a shared role for the library and what the stations share: a station's routes follow the role on that station. A shared route given a station to look at (`station_id` or `on_station_id` of `GET /tracks` and of a selection's `filters`, `station_id` of `GET /categories`, `GET /medleys` and `GET /tracks/{id}/plays/grid`) answers `404` for a station the user does not see, as that station's routes do.

Create the first admin on the command line (the password is read without echo; from stdin when it is not a terminal):

```sh
docker compose exec heron-api heron-api user add admin --role admin
docker compose exec heron-api heron-api user passwd admin
docker compose exec heron-api heron-api user list
docker compose exec heron-api heron-api user reset-2fa admin
```

`user reset-2fa` removes a user's [second factors](#two-factor-authentication) (authenticator app, passkeys and recovery codes), for someone who lost them all, the last admin included. The web interface has no such button.

Then:

- **Web app:** `POST /api/v1/auth/login` with `{"username", "password"}` sets a session cookie (or asks for a [second factor](#two-factor-authentication) first) (`heron_session`, `HttpOnly`, `SameSite=Strict`, `Secure` when the proxy reports HTTPS through `X-Forwarded-Proto`). Sessions last 30 days from the last use. Requests that change something with a cookie must also send an `X-Requested-With` header (any value): pages on other sites cannot add it, so they cannot act on your behalf.
- **Scripts:** create an API token (`POST /api/v1/tokens`, shown once) and send `Authorization: Bearer heron_…`. Tokens act as their user, with its role; they can expire and be revoked. They need no second factor.
- Passwords (8 characters at least) are hashed with Argon2id, one at a time so sign-ins never take CPU from the engine on a Raspberry Pi. After 5 failed sign-ins a username is locked for a minute. Only hashes of session and API tokens are stored. Changing a password signs the user out everywhere else; disabling a user ends their sessions and stops their tokens. The last enabled admin cannot be demoted, disabled or deleted.
- `GET /api/v1/status`, `/api/v1/library/status`, `/api/v1/openapi.json` and the [public endpoints](#public-endpoints) (`/api/v1/public/…`) need no sign-in. `/status` also reports the server clock's synchronization (`clock`, read from the kernel with a read-only `adjtimex`: containers share the host's clock), with a `warning` when it is not synchronized or may be off by more than a second; the web UI shows it on every page. It gives `backups`, what keeps backups from being written (the backup folder not writable: the system's `error`, and `permission_denied` when the folder is not the API's user's, `uid`:`gid`), shown to admins; and the API's `version` and, in the published images, the commit it was built from (`revision`): the web UI compares them with its own, and says so when they differ.

### Two-factor authentication

A user can add a second factor to their password. It is asked at sign-in only, after the password; there is no "trust this device". Sessions still last 30 days from the last use, and API tokens are not concerned.

- **Authenticator app** (TOTP, RFC 6238): HMAC-SHA-1, 6 digits, 30 seconds, one step of clock drift either side, for Aegis, Google Authenticator, 1Password, Bitwarden… One app per user. Each code is accepted once: the last time step used is kept.
- **Passkeys and security keys** (WebAuthn): Touch ID, Windows Hello, a phone, a security key with a PIN. Several per user, each with a name; each one says when it was last used. The browser only offers them over HTTPS, or on `localhost`, and on a host name (not an IP address). The relying party is the address the browser shows: its origin is the request's `Origin` header (browsers send it on every `POST`), or else `X-Forwarded-Proto` and `X-Forwarded-Host` (or `Host`); its ID is that origin's host name. A passkey only works on the host name it was added on.
- **Recovery codes:** 10 codes of 8 characters (`7KQ2-M9XD`), given when the first factor is set up and shown once. Each works once in place of a code. New ones void the old ones. Only their Argon2id hashes are stored.
- **Required per role:** an admin can require it for admins, operators and viewers (`PUT /settings/two-factor`, `{"admin": true, "operator": true, "viewer": false}`; off by default). With several stations, the user's highest role counts, shared or on any station. A user who must have it and has none gets a session that can only set it up: every other route answers `403` with the code `two_factor_required`, except `GET /auth/me`, `PUT /auth/preferences`, `POST /auth/logout` and the routes under `/auth/two-factor`. Each user has `two_factor` in `GET /auth/me` and `GET /users`: `totp`, `passkeys` (how many) and `required`.

Signing in with a second factor:

1. `POST /auth/login` with the username and password. Without a second factor, the session starts as before (`200`). With one, the answer is `202` with `{"second_factor": ["totp", "webauthn", "recovery"]}` (the factors the user has) and a pending sign-in cookie, `heron_pending`, that lasts 5 minutes; no session yet.
2. One of `POST /auth/login/totp` (`{"code"}`), `POST /auth/login/recovery` (`{"code"}`), or `POST /auth/login/webauthn/start` (the options for `navigator.credentials.get()`) then `POST /auth/login/webauthn/finish` (`{"credential"}`: the browser's answer, as JSON with base64url fields) starts the session, like a sign-in with a password.
3. A wrong code answers `422`, and counts with the password's failures: after 5, the username is locked for a minute (`429`). A missing or expired pending sign-in answers `401`.

Your second factors, under `/auth/two-factor`: setting one up or removing one asks for your password again (`{"password"}`), and signs you out everywhere else.

- `GET /auth/two-factor`: your app (when it was set up and last used), your passkeys (`id`, `name`, created, last used), how many recovery codes are left, and whether your role requires it.
- `POST /auth/two-factor/totp` (`{"password"}`): a new secret, with its `otpauth://` URI and a QR code (SVG), waiting 10 minutes for `POST /auth/two-factor/totp/confirm` (`{"code"}`), which turns it on. It replaces the app set up before.
- `POST /auth/two-factor/passkeys` (`{"password", "name"}`): the options for `navigator.credentials.create()`, then `POST /auth/two-factor/passkeys/finish` (`{"credential"}`) adds the passkey. `PATCH /auth/two-factor/passkeys/{id}` (`{"name"}`) renames it.
- `DELETE /auth/two-factor/totp`, `DELETE /auth/two-factor/passkeys/{id}` (`{"password"}`): removing the last factor turns two-factor authentication off and voids the recovery codes; it is refused (`409`) when your role requires it.
- `POST /auth/two-factor/recovery-codes` (`{"password"}`): 10 new codes, the old ones voided.
- The first factor set up answers with the 10 recovery codes (`recovery_codes`); later ones answer `null` there.

The TOTP secret has to be read to check codes, so it cannot be hashed: like the output passwords and the webhook secrets, it is kept in the database, and in the [backups](backups.md), which can be encrypted. Setting up, removing and resetting a factor, using a recovery code and generating new ones are written to the API's log.

## Routes

What belongs to a station (its engine and decks, queue, history, clocks, schedules, rotation log and simulations, outputs, webhooks, listener statistics, live WebSocket and its own settings: `playout`, `eq`, `agc`, `queue`, `rotation`, `station`) is served under **`/stations/{id}/…`**: `GET /stations/2/queue`, `PUT /stations/2/settings/playout`. The same routes without the prefix (`GET /queue`) serve the **first station**, so scripts and station websites written for one station keep working ([Several stations](stations.md#api)). Everything else is shared by the stations and has no prefix; a shared route under a station answers `404`.

| Area | Routes |
|---|---|
| Modules | `GET`, `PUT /modules` (the installation's, admins change them); `GET`, `PUT /stations/{id}/modules` (`installation`, `station` and `on`, what is on for the station; its admins change its own). A module turned off answers `404` (`module_off`) on its routes — see [Modules](modules.md#api) |
| Stations | `GET`, `POST /stations`; `GET`, `PUT`, `DELETE /stations/{id}` (`{"name", "code", "enabled", "position"}`; the answers add `engine_variable`, `engine_configured` and `engine_taken_by`, the code of the station that already has that engine): admins change them; see [below](#stations) |
| Account | `POST /auth/login`, `POST /auth/login/totp`, `/auth/login/recovery`, `/auth/login/webauthn/start`, `/auth/login/webauthn/finish`, `POST /auth/logout`, `GET /auth/me`, `PUT /auth/password`, `PUT /auth/preferences` (`{"theme", "language", "full_width"}`, each optional: the theme is system, light or dark; the language is system or a language tag such as `fr`; `full_width` drops the centred column); `GET`, `POST /tokens`, `DELETE /tokens/{id}`; `GET /auth/two-factor`, `POST /auth/two-factor/totp`, `/auth/two-factor/totp/confirm`, `DELETE /auth/two-factor/totp`, `POST /auth/two-factor/passkeys`, `/auth/two-factor/passkeys/finish`, `PATCH`, `DELETE /auth/two-factor/passkeys/{id}`, `POST /auth/two-factor/recovery-codes` (see [Two-factor authentication](#two-factor-authentication)) |
| About | `GET /about`: the build (version, `revision`, build date, each service's version and whether it answers) for every role; for admins also `runtime`, `dependencies` and `configuration` (the settings that are not secret; a secret is only `set` or not) |
| Users (admins) | `GET`, `POST /users`; `PATCH`, `DELETE /users/{id}`; `GET`, `PUT /settings/two-factor` (which roles require a second factor; every signed-in user can read it) |
| Backups (admins) | `GET /backups`; `POST /backups` (back up now); `GET /backups/{name}/config`, `GET /backups/{name}/database` (download); `DELETE /backups/{name}`; `POST /backups/{name}/restore` (`{"key"}` for an encrypted backup); `POST /backups/restore` (upload a configuration file, `.json` or `.json.age`, as the body, and restore it; the key of an encrypted one in the `X-Heron-Backup-Key` header); the report lists what was replaced and what was left out; each backup says whether it is `encrypted`, and with which `key`: `current`, `earlier` or `unknown` (an encrypted upload), with its public key (`recipient`) when it is known. Encryption: `GET /backups/key` (`state`: off, pending or on; the public key, when and by whom it was set, the earlier ones); `POST /backups/key` (`{}` generates a key and returns it, once; `{"recipient": "age1…"}` uses the admin's own), then `POST /backups/key/confirm` (`{"key"}`, typed back: turns encryption on, encrypts the folder, takes a backup); `POST /backups/key/check` (`{"key"}`: also tests the newest backup encrypted with it), `POST /backups/key/later`; `POST /backups/{name}/test` (`{"key"}` for an encrypted backup) and `GET /backups/tests` (the last test, and the last one that passed); each backup also says whether it is `damaged`; `DELETE /backups/key` (turns encryption off, or drops a pending key); `GET /backups/state`, for the banners: whether backups are encrypted, whether the admin's [key check](backups.md#checking-the-key) is due, how many backups are damaged, whether a test is due; see [Backups – Encryption](backups.md#encryption) |
| Engine and decks | `GET /engine` (reachable, configuration in sync), `GET /player` (decks, cued track, queue, outputs), `GET /player/levels`, `POST /player/skip`, `/player/pause`, `/player/resume`, `POST /player/fx` (`{"track_id"}`), `DELETE /player/fx` |
| Queue | `GET /queue` (the cued track first; a jingle of a [linked slot](rotation.md#linked-slots) has its track's entry in `linked_to`), `POST /queue` (`{"track_ids", "position"}`; a position between a linked jingle and its track puts the tracks before the jingle), `DELETE /queue`, `DELETE /queue/{entry_id}` (removing a linked track removes its jingle; removing the jingle keeps the track), `PUT /queue/{entry_id}/position` (a linked jingle and its track move together, the jingle first) |
| Library | `GET /library/status`; `GET /tracks` (`q`, `category_id`, `filter`: playable, missing, unanalyzed, silent, disabled, needs_review, no_cover; `labels_all`, `labels_any`, `labels_none` (comma-separated IDs); `no_category` (tracks in no category); `station_id` (play counts, last played, their sort and `never_played`, `not_played_days` on that station; on every station otherwise), `on_station_id` (only the tracks on that station: in one of its categories and in none it keeps out of its library, `kept_out_category_ids` of its `rotation` settings), `never_played`, `not_played_days`, `added_days`, `batch_id` (tracks an import created), `bitrate_below`, `era`, `mood`, `sound_code`, `genre`, `year_min`, `year_max`, `bpm_min`, `bpm_max`, `ratio_min`, `ratio_max` (play ratio in tenths, 0 to 10; a disabled track counts as 0; with `on_station_id`, the station's ratio, and a track disabled there counts as 0 too); `sort`: artist, title, album, genre, added, last_played, play_count, bitrate, duration, year, bpm, mood, sound_code, level_min, level_max, play_ratio, and `desc`; each track has its `level_min_db` and `level_max_db`, its `play_ratio` (the default, tenths, 1 to 10, kept while it is disabled), who disabled it and when (`disabled_by`, a username, and `disabled_at_unix_ms`; `null` while it is enabled, or for a user since deleted), with `on_station_id` its `station_ratio` there (see below), and its `cover_url` (`null` without a cover); `limit`, `offset`; `sort=play_ratio`, `filter=disabled` and `filter=playable` follow the station's ratio with `on_station_id`); `GET`, `PATCH /tracks/{id}` (`enabled`; `play_ratio`, 0 to 10 tenths, 0 disables the track and keeps its ratio; `reset_counters`: its [play ratio](rotation.md#play-ratio) counters back to 10 on every station; with `station_id`, the three apply to that station only, and `default_ratio: true` puts the station back on the default ratio, enabled there; the answer has its `plays`: count and last play in total and per station, and its `station_ratios`: on each station, see below); `PUT`, `DELETE /tracks/{id}/overrides`; `PUT /tracks/{id}/intro` (`{"intro_ms": 15200}`, `null` removes it: where the vocals start, for [jingles over the intro](audio-engine.md#jingles-over-the-intro)); `PUT /tracks/{id}/hook` (`{"hook_in_ms", "hook_out_ms"}`, in before out, both `null` to remove it); `PUT /tracks/{id}/categories`; `GET /tracks/categories?ids=1,2,3` (each track's categories, `id`, `name` and `color`, for up to 200 tracks: what the decks and the queue show); `GET /tracks/{id}/plays/grid` (`station_id`, every station's when left out; `days`, 1 to 365, 30 by default; `source`, e.g. `rotation`: when the track played, by day and by hour of the week, see [Plays by day and hour](#plays-by-day-and-hour)); `GET /tracks/{id}/waveform`; `GET /tracks/{id}/audio` (the file, with range requests, for previews); `GET`, `PUT`, `DELETE /tracks/{id}/cover`, `POST /covers/fetch`, `GET /covers/candidates`, `POST /covers/candidates/{id}/accept`, `POST /covers/candidates/refuse`, `GET /public/covers/{hash}` (see [Cover art](#cover-art)); `POST /playout/transition` (preview a transition between two tracks, optionally with unsaved settings, and unsaved overrides of either track: `from_overrides`, `to_overrides`) |
| Library management | `POST /library/musicbrainz-details` (`{"all"}`); `GET /library/review`; `POST /tracks/{id}/review/apply` (`{"release_id", "recording_id"}`, one of its candidates), `POST /tracks/{id}/review/keep`; `POST /library/organize` (dry run), `POST /library/organize/{plan_task_id}/apply`; `GET /library/batches`, `POST /library/batches/{id}/undo`; `GET /library/tasks`, `GET /library/tasks/{id}`; `POST /tracks/delete` (`{"track_ids"}`); `GET /library/trash`, `POST /library/trash/{batch_id}/restore`, `POST /library/trash/empty` — see [below](#library-management) |
| Play ratios | `POST /tracks/ratios/import?dry_run=true` (a CSV file as the body, see [Play ratios per station](#play-ratios-per-station)); `POST /rotation/counters/reset` (a station's route: every counter of the station back to 10) |
| Import | `POST /library/import/preview` (`{"rows"}`), `POST /library/import/apply` (`{"categories", "problems", "rows"}`), `GET /library/imports`, `POST /library/imports/{id}/undo`: admins, see [Importing from another software](import.md) |
| Buffer zone | `GET /staging`; `POST /staging/groups` (`{"name"}`); `POST /staging/uploads` (`{"group_id", "name", "size"}`), `GET /staging/uploads/{id}`, `PUT /staging/uploads/{id}?offset=` (a chunk, `application/octet-stream`); `POST /staging/files/{id}/choice` (`{"kind": "release", "release_id", "recording_id"}` or `{"kind": "own"}`); `POST /staging/files/tags` (`{"file_ids", "set", "reset"}`); `POST /staging/files/preparation` (`{"file_ids", "add_categories", "remove_categories", "add_labels", "remove_labels", "mood", "sound_code"}`); `GET /staging/files/{id}/audio`; `POST /staging/import`, `POST /staging/discard` (`{"file_ids"}`) — see [below](#buffer-zone) |
| Categories | `GET`, `POST /categories` (`station_id`: only those a station uses); `GET`, `PUT`, `DELETE /categories/{id}` (with its `type_id`, the separation windows it overrides and its own `dayparts` ([daypart separation](rotation.md#daypart-separation), 0 to 10; 0 or left out: off) and `daypart_days` (the same daypart kept out on the following days, 0 to 7; 0 or left out: off), and its `station_ids`: the first station when left out on creation, kept when left out on a change; untagging a station that uses it is refused, `409`, listing what uses it, see [Several stations](stations.md#categories)); `GET`, `POST /category-types`, `PUT`, `DELETE /category-types/{id}` (`{"name", "no_two_in_a_row", "sends_metadata"}`, see [Outputs – Metadata](outputs.md#metadata) and see [Never two in a row](rotation.md#never-two-in-a-row); a type in use cannot be deleted, `409`) |
| Medleys | `GET`, `POST /medleys` (`station_id`: only those tagged with a station); `GET`, `PUT`, `DELETE /medleys/{id}`; `POST /medleys/{id}/plan`, `POST /medleys/{id}/pick`, `POST /medleys/{id}/duplicate`, `GET /medleys/{id}/candidates`, `POST`, `GET /medleys/{id}/preview`, `POST /medleys/{id}/render` (see [Medleys](medleys.md)) |
| Labels | `GET`, `POST /labels`; `PUT`, `DELETE /labels/{id}`; `POST /labels/{id}/merge` (`{"into_id"}`) |
| Selections | `POST /tracks/labels` (`add`, `remove`), `POST /tracks/categories` (`add` with weights, `remove`), `POST /tracks/programming` (`mood`, `sound_code`, `bpm`; `null` clears), `POST /tracks/tags` (a library task), `POST /tracks/tag-overrides` and `POST /tracks/tag-overrides/clear` (tags kept in Heron's database), `POST /tracks/enabled` — see [below](#actions-on-a-selection) |
| Artists | `GET /artists` (`q` matches any name, `limit`, `offset`); `GET`, `PATCH /artists/{id}` (`name`, `musicbrainz_id`; the answer has its `plays` per station and in total, like a track's); `POST /artists/{id}/merge` (`{"artist_id"}`, merged into this one); `PUT /artist-names/{id}` (`{"artist_id"}` moves the name, `{}` splits it off); `PUT`, `DELETE /artists/{id}/similar/{other}`; `GET /artist-suggestions`, `POST /artist-suggestions/{id}` (`{"accept"}`); `POST /artist-suggestions/musicbrainz` (a library task) |
| Rotation | `GET`, `POST /clocks`; `GET`, `PUT`, `DELETE /clocks/{id}` (name, `looping` and slots, replaced as a whole; a slot's separation cannot override `dayparts` or `daypart_days`, `422`; a slot's `linked_to_next`, off by default, [links it to the next slot](rotation.md#linked-slots), and cannot be followed by another linked slot, `422`); `POST /clocks/preview` (`{"slots", "looping"}`, saved or not: expected starts, categories running short); `GET`, `PUT /schedule` (the default schedule's whole week); `GET /schedules` (each schedule with its periods and how many hours it paints, the one playing now and the next change: `active_id`, `next_change` with its time and schedule), `POST /schedules` (`name`, `copy_from`: a schedule whose hours it starts with), `GET`, `PUT`, `DELETE /schedules/{id}` (`name`, `hours` and `periods`, replaced as a whole; the default schedule cannot be deleted or given periods); `GET /restrictions` (`target`, `target_id`, `station_id`), `POST /restrictions`, `PUT`, `DELETE /restrictions/{id}` (`station_ids`: the stations it applies to, every one when empty); `GET /rotation/picks` (the rotation log: `before`, `track_id`, `limit`); `POST /rotation/simulate` (`hours`, `start_unix_ms`, `seed`, `clock_id`); a pick of the log names the tracks its ratio held back (`held_tracks`: `id`, `artist`, `title`) |
| History | `GET /history` (`before`, `after`, `track_id`, `exclude_fx`, `limit`) |
| Deck EQ | `GET`, `PUT /settings/eq`; `GET`, `POST /eq-presets`; `PUT`, `DELETE /eq-presets/{id}` — see [below](#deck-eq) |
| AGC | `GET`, `PUT /settings/agc`; `GET`, `POST /agc-presets`; `PUT`, `DELETE /agc-presets/{id}` — see [below](#agc) |
| Settings | `GET`, `PUT /settings/playout`, `/settings/analysis`, `/settings/queue`, `/settings/rotation` (with `daypart_count`, 4 or 6: the station's dayparts; the [daypart separation](rotation.md#daypart-separation) is each category's), `/settings/station` (time zone, `public_url`, `gateway_url`), `/settings/programming`, `/settings/statistics`, `/settings/medleys`; `GET`, `PATCH /settings/library`; `GET`, `PUT`, `DELETE /settings/station/logo` (the [station logo](library.md#the-station-logo), admins change it) |
| Outputs | `GET`, `POST /encoder-profiles`; `GET`, `PUT`, `DELETE /encoder-profiles/{id}`; `GET`, `POST /outputs`; `GET`, `PUT`, `DELETE /outputs/{id}` (with each output's live state, and its `statistics` settings: the stream server's admin credentials) |
| Webhooks (admins) | `GET`, `POST /webhooks`; `GET`, `PUT`, `DELETE /webhooks/{id}`; `POST /webhooks/{id}/secret`; `POST /webhooks/{id}/test`; `GET /webhooks/{id}/deliveries` (`before`, `status`, `limit`); `POST /webhook-deliveries/{id}/retry` — see [below](#webhooks) |
| Listener statistics | `GET /listeners/now`, `/listeners/samples`, `/listeners/days`, `/listeners/hours-of-day`, `/listeners/tracks`, `/listeners/players` (CSV with `format=csv` for days, hours and tracks); `GET`, `PUT /settings/statistics` (admins) — see [Listener statistics](statistics.md#api) |
| Shows | `GET /shows` (each [show](listener-interaction.md) on for the station: its settings and whether it is open now); `GET`, `PUT /shows/{module}` (`dedications`, `match`, `top30`, `likes`: hours, website, notifications, see [below](#shows); operators change them; `GET` also lists the station's webhooks by name, to choose from) |
| Likes | `GET /likes/current`, `/likes/week`, `/likes/tracks/{id}` — see [below](#likes) |
| Dedications | `GET`, `PUT /dedications/settings`; `GET /dedications`; `POST /dedications/{id}/accept`, `/dedications/{id}/refuse` — see [below](#dedications) |
| Match | `GET`, `PUT /match/settings`; `GET /match/current`; `POST /match/current/close`, `/match/current/cancel`; `GET /match/history` — see [below](#match) |
| Top 30 | `GET`, `PUT /top30/settings`; `GET /top30/chart`, `/top30/charts`; `POST`, `DELETE /top30/charts/{week}/removals/{track_id}`; `PUT /top30/charts/{week}/new-release`; `GET /top30/show` — see [below](#top-30) |
| Gateway | `GET /gateway/state`, and each show's writes under `/gateway/stations/{code}/{module}/…`: for `heron-public` only, with the gateway token, see [below](#the-public-gateway) |
| Public | `GET /public/now-playing`, `GET /public/history` (`before`, `limit`), `GET /public/queue`, and the same per station under `/public/stations/{code}/…` — no sign-in, see [below](#public-endpoints) |
| Live | `GET /live` (WebSocket) |

Every engine action is there: queue (enqueue, remove, clear, move, and replace, which the queue manager uses), transport (skip, pause, resume), FX player, state, levels, track changes (WebSocket), playout settings, outputs, the deck EQ and the AGC chain. Tracks the engine plays from its fallback directory show with the source `fallback`; autofill does not count them as queued, and anything queued replaces a cued one.

## Stations

Each station has a name, a code (1 to 32 lowercase letters and digits, unique), whether it is enabled, and a position in lists. Its engine's gRPC address is in `heron-api`'s environment: `HERON_ENGINE_URL` for the first station, `HERON_ENGINE_URL_<CODE>` for the others (`engine_variable`; `engine_configured` says whether it is set). Their code is therefore set once: changing it is refused (`409`). The first station is the one an existing Heron became: it is named from `HERON_STATION_NAME` until renamed, its code can be changed, and it can be neither disabled nor deleted.

- A new station needs its own engine, a new service of the deployment, and its variable in `heron-api`'s environment ([Upgrading – Several stations](upgrading.md#several-stations)); the API connects to it when the station is added, or when it starts with the variable set, and starts its queue manager, history and webhooks.
- A disabled station is not fed: its engine plays what it has queued, then its fallback directory. Its programming can still be edited.
- A station is deleted only once it has no clocks, schedule, outputs or webhooks left; its history, rotation log and listener totals go with it.

## Live WebSocket

`/api/v1/live`, signed in like any request (the browser sends the session cookie). Each message is a `LiveMessage` (see the OpenAPI document), told apart by `type`:

| `type` | When | Content |
|---|---|---|
| `state` | On connection, then whenever the decks, the queue, the outputs or the engine's reachability change | `player`: the same as `GET /player`, `null` while the engine is unreachable |
| `positions` | Twice a second | Each deck's position and length |
| `levels` | Ten times a second, unless the URL has `?levels=false` | Peak and RMS (dBFS) of the master bus, after the AGC chain, and of each deck; while the chain is on, `agc`: the peak and RMS of its `input` (the master bus before it), the AGC's gain, and the gain reduction (dB) of each band and of the final stage |
| `agc` | Whenever the AGC settings change | `settings`: the same as `GET /settings/agc` |
| `eq` | Whenever the deck EQ settings change | `settings`: the same as `GET /settings/eq` |
| `system` | On connection, then every 3 seconds; **operators and admins only**, viewers never receive it | `cpu_percent` and `mem_percent` (0 to 100), `load_avg` (1, 5 and 15 minutes) and `cores`, of the machine the API runs on. Under Docker, the load average is the host's while the CPU and memory may be the container's ([Monitoring](monitoring.md#what-else-could-be-monitored)) |
| `track` | Each track change | `event`: `started` (with `resumed`: the track on air going on in a restarted engine, or one taking over, not a new play) or `ended` (with `played_ms` and `reason`: finished, skipped, stopped, replaced) |

The API polls the engine's state twice a second and its levels ten times a second, only while someone watches them.

## Webhooks

Webhooks tell other systems (a website, a chat bot, a studio display) about track changes as they happen. Admins manage them; their secrets are never sent back.

- **Events:** `track.started`, `track.ended` and `track.skipped`, as the engine reports them: a skip is `track.skipped` (not also `track.ended`), any other end (played to its transition, an FX stopped or replaced) is `track.ended`. The track on air going on in a restarted engine, or in one taking over from another (an upgrade), is not sent again. Tracks of the FX player, and tracks that send no metadata (a category type with `sends_metadata` off: jingles, ads), are only sent to webhooks with `include_fx`; tracks from the fallback directory are sent like any other.
- **`queue.changed`:** what comes next changed: a track started (the queue moved on), autofill or an operator queued, removed, moved or replaced one, the queue was cleared. It carries the **upcoming tracks in airing order**, at most 10, the music only (as the public endpoints: no FX player, no tracks that send no metadata such as jingles, ads and medleys), each with its position and planned start (as the Queue page shows it, without crossfades). Changes in a burst are sent once, 2 seconds after the last; nothing is sent when the list a listener would see did not change (a planned time moving after a skip is not a change on its own, but comes with the next one).

  ```json
  {"event": "queue.changed", "delivery_id": 43, "at_unix_ms": 1792917865323,
   "track": null, "played_ms": null, "reason": null,
   "upcoming": [{"position": 0, "artist": "Artist", "title": "Title", "album": null,
                 "duration_ms": 212000, "planned_start_unix_ms": 1792918037000}]}
  ```
- **Show events:** `show.upcoming`, `show.started` and `show.ended`, chosen per show on its page, not in the webhook's `events` (which refuses them, `422`): they go to the webhooks the [show](listener-interaction.md#notifications) names. Their body has `show` (`module`, `name`, `link`, `end_unix_ms`, `next_unix_ms`) and `message`, the show's message written out; `track` is `null`.
- **Covers:** each track (and each upcoming track) has a `cover_url`, as the [public endpoints](#public-endpoints) give it: its cover, else the station's logo, else `null`. Set the station's public address so that it is a full address, which Discord and Slack need.
- **Body:** by default the JSON `Payload` of the OpenAPI document:

  ```json
  {"event": "track.started", "station": "main", "delivery_id": 42, "at_unix_ms": 1792917865323,
   "track": {"track_id": 17, "artist": "Artist", "title": "Title", "album": "Album",
             "cover_url": "https://radio.example.org/api/v1/public/covers/9f2c…",
             "duration_ms": 215000, "deck": "a", "source": "rotation"},
   "played_ms": null, "reason": null}
  ```

  Or a **body template** of your own, with its content type, for receivers that want a format of their own (Discord, Slack, a display). Placeholders: `{{event}}`, `{{station}}` (the code of the webhook's station), `{{delivery_id}}`, `{{at_unix_ms}}`, `{{track_id}}`, `{{artist}}`, `{{title}}`, `{{album}}`, `{{cover_url}}`, `{{duration_ms}}`, `{{deck}}`, `{{source}}`, `{{played_ms}}`, `{{reason}}`; for `queue.changed`, `{{upcoming_json}}` (the array, as JSON, inserted as it is: `{"next": {{upcoming_json}}}`), `{{next.artist}}`, `{{next.title}}`, `{{next.album}}`, `{{next.cover_url}}` and `{{next.planned_start_unix_ms}}` for the first upcoming track; `{{message}}` (a show event's message, `Artist – Title` for a track event), `{{show.module}}`, `{{show.name}}`, `{{show.link}}` (spaces inside the braces are allowed). When the content type is JSON, values are escaped to fit between quotes: `{"content": "Now playing: {{artist}} – {{title}}"}`. Missing values are empty; unknown placeholders are refused (`422`).
- **Requests:** `POST` to the webhook's URL (`http` or `https`; it may be an address of your network: only admins can set it), with `X-Heron-Event`, `X-Heron-Delivery` (the delivery's ID), `X-Heron-Timestamp` (Unix seconds) and `X-Heron-Signature`. Redirects are not followed.
- **Signature:** `X-Heron-Signature: sha256=<hex>`, the HMAC-SHA256 of `"{timestamp}.{body}"` with the webhook's secret (`whsec_…`, shown once when the webhook is created or given a new secret with `POST /webhooks/{id}/secret`). Receivers should compare it in constant time and refuse old timestamps. In Python:

  ```python
  expected = "sha256=" + hmac.new(secret.encode(), f"{timestamp}.{body}".encode(), hashlib.sha256).hexdigest()
  ok = hmac.compare_digest(expected, request.headers["X-Heron-Signature"])
  ```

- **Delivery and retries:** deliveries are sent as soon as the engine reports the change, at most 8 at a time. A delivery that does not get a `2xx` within 10 seconds is retried after 10 s, 30 s, 2 min, 10 min, 30 min, 1 h and 3 h, then marked `failed` (8 attempts). Deliveries are rows in the database, so retries survive a restart of the API; one being sent when the API stopped is sent again a minute later, so receivers may see a delivery twice (same `X-Heron-Delivery`). Deliveries of a disabled webhook that were still pending fail.
- **Delivery log:** `GET /webhooks/{id}/deliveries` lists what was sent, its status (`pending`, `delivered`, `failed`), attempts, the receiver's status code, the error and the start of its answer (1 KiB). `POST /webhooks/{id}/test` sends a `ping` (even to a disabled webhook); `POST /webhook-deliveries/{id}/retry` sends a delivered or failed delivery again, with every attempt again. Deliveries are kept 30 days.

## Shows

Each [show](listener-interaction.md) has its settings per station, `GET`, `PUT /stations/{id}/shows/{module}` (operators change them; the module must be on, else `404`):

```json
{"enabled": true,
 "hours": [{"days": [1, 2, 3, 4, 5], "start_hour": 17, "end_hour": 18}],
 "website": {"title": "Your dedications", "color": "#c62828", "small_widget": true,
             "closed": "message", "closed_message": "Dedications are back {day} at {hour}",
             "page_url": "https://radio.example.org/dedications"},
 "notifications": {"webhook_ids": [3], "lead_minutes": 60, "language": "en",
                   "events": {"show.upcoming": {"enabled": true, "message": "{name} opens in an hour: {link}"},
                              "show.started": {"enabled": true, "message": "{name} is open until {end}: {link}"},
                              "show.ended": {"enabled": false, "message": "{name} is closed. Back {next}"}}}}
```

- `hours`: none for [Likes](#likes), which are open all the time (`422`). Days 1 (Monday) to 7 (Sunday), `start_hour` 0 to 23, `end_hour` after it, up to 24, at most 50 ranges, in the station's time zone. Hours another show of the station has are refused, `409`, naming the show and the hours (`18:00–19:00 on Saturday belongs to the match`).
- `website`: `title` (60 characters at most; empty: the show's name in the listener's language), `color` (`#rrggbb`), `small_widget`, `closed` (`message` or `nothing`), `closed_message` (200 characters at most, with `{day}` and `{hour}`; empty: the widget's own sentence), `page_url` (the website's page of the show, for `{link}`).
- `notifications`: `webhook_ids` (webhooks of the station), `lead_minutes` (1 to 1440), `language` (`en`, `fr`, `es`, `de`), and each event with `enabled` and `message` (500 characters at most, with `{name}`, `{link}`, `{end}`, `{next}`, and the show's own placeholders; an unknown one is refused, `422`).

`GET` answers `module`, `settings` (as above), `status` (`open` now, `closes_at_unix_ms` while open, `next_opening_unix_ms`), `timezone` and `webhooks` (`id`, `name` of the station's webhooks), and `PUT` answers the same. `GET /stations/{id}/shows` lists the shows on for the station, with the same answer, for the week grid of each show's page.

## Public endpoints

For station websites, apps and displays: no sign-in, readable from any origin (`Access-Control-Allow-Origin: *`, `GET` only, no credentials), the music only (decks A and B, not the FX player, and not tracks that send no metadata: while a jingle is on air, now-playing shows the song before it), without IDs or paths.

- `GET /api/v1/public/now-playing`: `now` (artist, title, album, `cover_url`, `started_at_unix_ms`, `duration_ms`, `position_ms`, `dedication`: `from`, `to` and `message` of an accepted [dedication](#dedications), else `null`, and `match`: `name`, `percent` and `votes` of the [match](#match) it won, else `null`), `null` while nothing plays or the engine is unreachable, and `server_time_unix_ms`. Cached 5 seconds (`Cache-Control: public, max-age=5`).
- `GET /api/v1/public/queue`: `upcoming`, the tracks waiting in airing order, as `queue.changed` sends them (at most 10, the music only, with `position` and `planned_start_unix_ms`), and `server_time_unix_ms`; empty while the engine is unreachable. Cached 5 seconds.
- `GET /api/v1/public/history`: the latest plays, most recent first (artist, title, album, `cover_url`, `started_at_unix_ms`, `ended_at_unix_ms`); `limit` 1 to 50 (20), `before` for the next page. Cached 15 seconds.

- `GET /api/v1/public/covers/{hash}` (`size`: 600, the default, or 96): a cover or a station logo, as JPEG. Its address changes with the image, so it is cached for a year (`Cache-Control: public, max-age=31536000, immutable`, with an `ETag`).

`cover_url` is the track's cover, else the station's logo, else `null`. It is a full address when the station's settings have its public address (`public_url`, such as `https://radio.example.org`, the address `heron-web` is reached at from outside), a path (`/api/v1/public/covers/…`) otherwise: set it for websites on another address, and for webhooks. Add `?size=96` for a thumbnail.

Each station has its own, by its code: `/api/v1/public/stations/{code}/now-playing`, `/queue` and `/history`; the routes without a code are the first station's. They answer through `heron-web` like the rest of the API (`http://<host>:8000/api/v1/public/now-playing`). Put a cache or rate limit in the reverse proxy in front of it if the station is busy, or use the gateway, which serves them from its cache.

## The public gateway

`heron-public` serves listeners, under `/api/v1/public/` like the public endpoints, on its own port (`HERON_PUBLIC_PORT`, 8001 by default). See [Listener interaction](listener-interaction.md) for what it checks and why.

| Route | What it gives |
|---|---|
| `GET /api/v1/public/stations/{code}/now-playing`, `/queue`, `/history` | The API's public endpoints, from the gateway's cache |
| `GET /api/v1/public/stations/{code}/shows` | The station's shows that are on: each with `module`, `title`, `color`, `small_widget`, `closed`, `closed_message`, `page_url`, `hours`, `timezone`, `open`, `closes_at_unix_ms`, `next_opening_unix_ms` and `state` (the show's own live data, `null` until the show adds it) |
| `GET /api/v1/public/stations/{code}/{module}` | One of them (`404` when it is off) |
| `GET /api/v1/public/stations/{code}/events` | Server-Sent Events: `state` (`now_playing`, `shows`, `server_time_unix_ms`) when connecting and at each change; a comment every 15 seconds keeps the connection open |
| `GET /api/v1/public/stations/{code}/{module}.js` | The script of the show's page and small widget |
| `GET /api/v1/public/stations/{code}/{module}.html` | The show's page alone |
| `GET /api/v1/public/challenge` | A proof-of-work challenge: `challenge`, `bits`, `expires_at_unix_ms` |
| `POST /api/v1/public/stations/{code}/{module}[/{action}]` | A show's write (a dedication, a vote), with `X-Heron-Proof: <challenge>:<nonce>`; each show documents its own |
| `GET /api/v1/public/stations/{code}/{module}/{read}` | A show's own reads, from the API, cached a few seconds per query: the [dedications](#dedications)' `search` and `follow` |

Errors are those of the API: `403` (`forbidden`: no valid proof, or a website not allowed), `404` (unknown station or show, or a show turned off), `409` (`conflict`: the show is closed), `422` (`invalid`: a field too long or unknown), `429` (`too_many_requests`, with `Retry-After`), `503` (the API not reached, or no gateway token).

Behind it, the gateway calls the API with `Authorization: Bearer <HERON_PUBLIC_TOKEN>`:

- `GET /api/v1/gateway/state`: every enabled station (`code`, `name`, `timezone`, `now_playing` as the public endpoint gives it, `shows`: the shows on for it), and `server_time_unix_ms`. Read once a second.
- `GET /api/v1/gateway/stations/{code}/{module}/…`: a show's own reads (the dedications' search and follow link), with the listener's query.
- `POST /api/v1/gateway/stations/{code}/{module}/…`: a show's writes, with `X-Heron-Device` (the device's hash). The API checks again that the show is on (`404`) and open (`409`). A write the gateway adds up arrives as `{"counts": [{"fields": {…}, "count": 12}]}`; one it keeps per device as `{"writes": [{"device": "…", "fields": {…}}]}`, each device's last.

These routes accept only the gateway token, and the gateway token reaches nothing else: it is not a user's, and `HERON_PUBLIC_TOKEN` unset refuses it (`401`).

## Likes

Listeners' [likes](listener-interaction.md#likes) on the tracks played, per station (signed in; `404` while the `likes` module is off):

- `GET /stations/{id}/likes/current`: the play listeners can vote on now, `play_id`, `track_id`, `likes`, `dislikes`; `null` while nothing can be voted on (a jingle, nothing on air).
- `GET /stations/{id}/likes/week`: the most liked and least liked tracks of the week so far, from Monday 00:00 in the station's time zone: `from_unix_ms`, `min_votes` (10), and `most_liked`, `least_liked`, 10 each (`track_id`, `artist`, `title`, `likes`, `dislikes`, `share` from 0 to 1, `plays`: its plays with votes), among the tracks with at least `min_votes` votes. Ties go to the track with the most votes.
- `GET /stations/{id}/likes/tracks/{track_id}`: a track's votes on the station: `week`, `days_30` and `total` (each `likes`, `dislikes`), `min_votes`, and `plays`, its latest 20 plays with votes (`play_id`, `started_at_unix_ms`, `likes`, `dislikes`).

On the track list, `GET /tracks` gives each track's `likes` and `dislikes` over `likes_days` days (1 to 3650, 30 by default), on the station of `station_id`, else of `on_station_id` (every station's when neither is given), and takes:

- `sort=liked`: by share of likes, lowest first (`desc=true`: highest first); tracks below 10 votes come after those ranked, those without votes last;
- `liked_min`, `liked_max`: the share of likes in percent, 0 to 100 (tracks without votes are left out);
- `votes_min`: at least this many votes.

Each play of `GET /tracks/{id}/plays/grid` has its `likes` and `dislikes`.

Votes are counted per play (`play_votes`: the play, its station, its track, when it started, likes and dislikes), kept as long as the history: a period's ranking adds up the plays that started in it, per track and per station.

## Dedications

[Dedications](listener-interaction.md#dedications) on a station (signed in; operators decide and change the settings; `404` while the `dedications` module is off):

- `GET`, `PUT /stations/{id}/dedications/settings`: `max_per_range` (requests per opening range, everyone together, 1 to 1000, 30 by default), `per_device` (per device per range, 1 to 10, 1), `message_max` (characters, 20 to 500, 140), `retention_days` (1 to 365, 30), `category_ids` (the requestable categories, the station's; none: its music). The hours, website and notifications are the show's settings, [above](#shows).
- `GET /stations/{id}/dedications` (`status`: `pending`, `accepted`, `played` or `refused`; `limit` 1 to 200, 100 by default): the requests of the opening range on now, else of the last one, the latest first: `id`, `track_id`, `artist`, `title`, `from`, `to`, `message`, `status`, `reason`, `submitted_at_unix_ms`, `decided_at_unix_ms`, `decided_by`, `planned_at_unix_ms`, `played_at_unix_ms`, `play_id`, and for a waiting one `checks` (what the rules say now: each `kind`, `rules_kept`, `artist_played`, `title_played`, `track_played`, `in_queue`, `restriction` or `unplayable`, with `minutes_ago`, `separation_min`, `at_unix_ms` or `restriction_id` as it applies, and `blocking`: a restriction or a track that cannot play). With them: `counts` (`pending`, `accepted`, `played`, `refused`, `received` in the range), `max_per_range`, the range's `opens_at_unix_ms` and `closes_at_unix_ms` (`null` without one), and `on_air`, the dedication of the track playing (`null` without one).
- `POST /stations/{id}/dedications/{id}/accept` with `{"place": "next"}` (`next`, `hour` or `best`): queues the track (source `request`) and answers the request, `planned_at_unix_ms` filled. `409` for a request not waiting, a track a restriction forbids then or that cannot play; `503` while the engine is unreachable.
- `POST /stations/{id}/dedications/{id}/refuse` with `{"reason": "just_played"}` (`null`, `just_played`, `unavailable` or `inappropriate`): `409` for a request not waiting.

Through the gateway (`docs/listener-interaction.md#dedications`), with the proof of work for the write:

- `GET /api/v1/public/stations/{code}/dedications/search?q=neon`: the requestable tracks whose artist or title has `q` (2 to 60 characters), 8 at most: `track_id`, `artist`, `title`. `409` while closed. Cached 30 seconds per query.
- `POST /api/v1/public/stations/{code}/dedications`: `{"track_id": 42, "from": "Marc", "to": "Julie", "message": "Happy birthday!"}` (`from` 40 characters, `to` 60, `message` the station's maximum, 500 at most in the gateway). Answers `201` with the request as its follow link gives it, and `follow`, its key. `422` for a track that cannot be requested or a message too long, `409` while closed or once the range is full, `429` past the device's requests in the range or while one of its requests waits.
- `GET /api/v1/public/stations/{code}/dedications/follow?id=<follow>`: `status` (`pending`, `accepted`, `played`, `refused`), `reason` (`closed`, `just_played`, `unavailable`, `removed`; `null` for no reason or an inappropriate message), `artist`, `title`, `from`, `to`, `message`, `submitted_at_unix_ms`, `decided_at_unix_ms`, `planned_at_unix_ms`, `played_at_unix_ms`. `404` once deleted. Cached 5 seconds.

The show's own notification events are `dedication.played` and `dedication.accepted`. Its public state (`state` in the gateway's state): `remaining` (requests left in the range, `null` while closed), `message_max`, `per_device`, `retention_days` and `today` (the dedications accepted or played since midnight, in planned order: `artist`, `title`, `from`, `to`, `message`, `at_unix_ms`, `played`). `now-playing` and the `track.started` webhook carry the track's `dedication` (`from`, `to`, `message`), `null` without one; webhook templates have `{{dedication_from}}`, `{{dedication_to}}` and `{{dedication_message}}`.

## Match

The [Match](listener-interaction.md#match) of a station (signed in; operators act and change the settings; `404` while the `match` module is off):

- `GET`, `PUT /stations/{id}/match/settings`: `every_tracks` (a winner every N tracks, 2 to 20, 4 by default), `category_id` (the category the two tracks come from, the station's; `null`: no match), `jingle_category_id` (the announcement over the vote's track's intro, the station's; `null`: none), `close_before_s` (seconds before the next transition the vote closes, 5 to 60, 10), `live_results` (the votes on the website as they come, `true`), `loser_same_day` (a loser can be a candidate again the same day, `false`). The hours, website and notifications are the show's settings, [above](#shows).
- `GET /stations/{id}/match/current`: the match queued or open, else `null`: `id`, `status` (`queued`, `open`), `category_id`, `category_name`, `vote_track` (`track_id`, `artist`, `title`), `jingle` (the same, or `null`), `choices` (two: `choice` `a` or `b`, `track_id`, `artist`, `title`, `votes`, `percent`), `votes`, `opened_at_unix_ms`, `closes_at_unix_ms` and `transition_at_unix_ms` (once open).
- `POST /stations/{id}/match/current/close`: closes the vote now and decides; `POST /stations/{id}/match/current/cancel`: cancels the match, the first track staying in the queue as an ordinary track. Both answer the match as it ends; `404` without a match, `409` for a vote not open (close) or a match already decided, `503` while the engine is unreachable.
- `GET /stations/{id}/match/history` (`limit` 1 to 200, 50 by default, `before` an ID for the next page): the matches decided or cancelled, the latest first: `id`, `status` (`closed`, `cancelled`), `opened_at_unix_ms`, `closed_at_unix_ms`, `winner` and `loser` (`track_id`, `artist`, `title`, `votes`), `votes`, `percent` (the winner's share), `decided_by` (`votes`, `tie`, `no_votes`, `early`: closed by an operator). With them, `week`: the week's `matches` and `votes` (from Monday 00:00, station time).

Through the gateway, with the proof of work for the write:

- `GET /api/v1/public/stations/{code}/match`: the show, as every show, with its live `state`: `match_id` (the match open to votes, `null` between two), `closes_at_unix_ms`, `live_results`, `choices` (two: `choice`, `artist`, `title`, `cover_url`, and with live results `votes` and `percent`, else `null`), `votes` (`null` without live results), `last` (the last result: `winner` and `loser` with `artist`, `title`, `cover_url`, `percent`, `votes`, `decided_by`, `closed_at_unix_ms`) and `today` (today's results, the latest first, the same way).
- `POST /api/v1/public/stations/{code}/match/vote`: `{"match_id": 12, "choice": "a"}` (`a` or `b`). `202`; `409` for a match that is not open any more. A device's last vote counts.

Behind it, the gateway sends each device's last vote every second to `POST /api/v1/gateway/stations/{code}/match/vote` as `{"writes": [{"device": "…", "fields": {"match_id": 12, "choice": "a"}}]}`: the votes for the match open count, the others are dropped (`counted`, `dropped`).

The show's own notification events are `match.started` and `match.closed`. The devices' votes (`match_votes`) are deleted an hour after their match closes, a day after the vote at the latest; the matches (`matches`) are kept.

## Top 30

The [Top 30](listener-interaction.md#top-30) of a station (signed in; operators change it; `404` while the `top30` module is off). A week is named by its Monday, `2026-11-02`.

- `GET`, `PUT /stations/{id}/top30/settings`:

  ```json
  {"close_day": 7, "close_hour": 17, "min_votes": 10, "size": 30,
   "category_ids": [], "jingle_category_id": 12, "new_release_category_id": 15}
  ```

  `close_day` 1 (Monday) to 7, `close_hour` 0 to 24 (Monday needs 1 or more: the week starts on Monday 00:00), `min_votes` 1 to 1000, `size` 1 to 50, `category_ids` the eligible categories (empty: the station's music), the jingle and new release categories or `null`. A category that is not the station's is refused, `422`. The show's hours, website and notifications are its [show settings](#shows).
- `GET /stations/{id}/top30/chart` (`week`: a past week; this week by default): `week`, `from_unix_ms`, `closes_at_unix_ms`, `frozen` (closed and kept; else the chart as it stands now), `votes` (over the week, eligible tracks), `entries` (each `rank`, `track_id`, `artist`, `title`, `likes`, `dislikes`, `share` from 0 to 1, `plays`, `movement`: `up`, `down`, `same`, `new` or `reentry`, `previous_rank`, `weeks`), `removed` (`track_id`, `artist`, `title`, `removed_by`, `removed_at_unix_ms`), `new_release` (`track_id`, `artist`, `title`, `proposed`: not chosen by hand; `null` without a new release category) and `locked` (a show was built from it: no change any more). Ranks follow the shares as they are when it is read, until it is frozen.
- `GET /stations/{id}/top30/charts` (`before`, a week, for the next page; `limit` 1 to 100, 20 by default): the frozen charts, the latest first: `week`, `from_unix_ms`, `closes_at_unix_ms`, `size` and `number_one` (`track_id`, `artist`, `title`).
- `POST /stations/{id}/top30/charts/{week}/removals/{track_id}`: takes the track out of the week's chart, the tracks after it moving up; `DELETE` puts it back. Refused, `409`, once a show was built from the chart, or for a week to come. Answers the chart.
- `PUT /stations/{id}/top30/charts/{week}/new-release`: `{"track_id": 42}` chooses the new release (a track of the new release category, else `422`), `{"track_id": null}` goes back to the proposed one. `409` once a show was built from the chart. Answers the chart.
- `GET /stations/{id}/top30/show`: the show on now, else the next one within a week (`null` without hours): `opens_at_unix_ms`, `closes_at_unix_ms`, `week` (the chart it plays; `null` without one), `built` (its programme is built: it is the one on air), `items` in order (`kind`: `jingle`, `track` or `new_release`; `rank`, `track_id`, `artist`, `title`, `length_ms`, `planned_at_unix_ms`, `skipped`: why it is passed over, or `null`; `queued`), `left_out` (the ranks left out to fit the slot), `music_ms`, `jingles_ms`, `new_release_ms` and `total_ms`. Before it is built, it is what would be built now.

The show's own notification events are `top.published` and `top.rank` ([Top 30 – Notifications](listener-interaction.md#top-30)). Its public state (`state` in the gateway's state): `chart` (`week`, `entries`: each `rank` and, once revealed, `artist`, `title`, `movement`, `previous_rank`, `weeks`, `on_air`; hidden ones have only their `rank` and `hidden: true`), `new_release` (`artist`, `title`, `hidden` until it plays), `live` (the show is playing it), `on_air_rank`, and `votes_count_for_unix_ms` (the opening the votes of now count for; `null` between the close and Monday). `chart` is `null` before the first show.

## Cover art

How covers are found and kept: [Library management – Cover art](library.md#cover-art).

- `GET /tracks/{id}/cover` (`size`: 600 or 96): the track's cover as JPEG, `404` when it has none (the web UI then shows a placeholder). Signed in. Its address stays the same when the cover changes, so it is revalidated (`ETag`, `Cache-Control: private, no-cache`): a `304` while it has not changed. Lists and pages use the `cover_url` of the track instead, which changes with the image and is cached for a year.
- A track's page (`GET /tracks/{id}`) has its `cover`: `source` (hand, embedded, file, cover_art_archive), `detail` (the file's name, or the release ID), `scope` (album or track), `width` and `height` of the image as found; and `cover_candidates`, its album's candidates waiting on the review screen.
- `PUT /tracks/{id}/cover?scope=album|track` (operators): sets the cover by hand, for the track's album (the default) or for the track only. The body is the image (`image/jpeg`, `image/png` or `image/webp`, at most 10 MB), or `{"url": "https://…"}`: the API fetches the image (`http` or `https`, 10 MB, 15 seconds). `422` when the image cannot be read, or for an album when the track has none. A cover set for the album replaces the exceptions of its tracks.
- `DELETE /tracks/{id}/cover?scope=album|track` (operators): removes the album's or the track's own cover, whatever its source; the file and its folder are looked at again for the next source.
- `POST /covers/fetch` (operators, `{"selection"}`: the tracks of a [selection](#actions-on-a-selection); `{}`: every album without a cover; `"candidates": true` to have the results shown as candidates even when one is sure): queues a `fetch_covers` library task (`202`). Albums with a cover are left out.
- `GET /covers/candidates`: the albums whose search was unsure, with a track of theirs, how many tracks they have and their candidates (`release_id`, title, artist, date, country, `score`, `thumbnail_url`). `POST /covers/candidates/{id}/accept` (`{"track_id"}` to give it to that track only) queues an `apply_cover` task, which fetches it and refuses the album's other candidates; `POST /covers/candidates/refuse` (`{"album_key"}`) refuses them all, and leaves the album without a cover. Operators.

## Queue manager

The engine holds the queue; the API feeds it and edits it.

- Tracks are sent with everything the engine needs, from the database: path, tags, cue points and crossfade point from the analysis, loudness, and the per-track overrides (cue points, Gap Killer, crossfade). Tracks whose file is missing, analyzed with nothing audible, or disabled, are refused. Tracks not analyzed yet play whole.
- **Autofill** (`queue` settings, on by default): when fewer than `min_queued` tracks wait (3), it queues the tracks [rotation](#rotation) picks among the enabled, analyzed, audible tracks. Turned off, the station plays silence once the queue is empty.
- **Positions** count the waiting tracks: 0 is next (before the cued track), 1 right after it, and so on.
- **Following the library:** every 5 seconds, waiting tracks are checked against the database. A track the librarian moved, or whose cue points changed, is updated in place (same entry and position); a track whose file disappeared, or that was deleted or disabled, leaves the queue. The track on air plays on (its file is already open).
- Each track carries its source (`rotation` for autofill, `manual` when queued by hand), recorded in the history.
- **Linked slots:** a pick of a [linked slot](rotation.md#linked-slots) comes with the next slot's track; both are queued in one call, the jingle linked to its track. The engine keeps the link: removing the track removes the jingle (a track leaving the library too), moving either moves both, and nothing is queued between them.

## Rotation

Autofill asks `heron-rotation` for each track it adds ([Rotation](rotation.md)):

- The station is loaded from the database each time: playable tracks, their artists and categories, clocks, the schedules and their periods, enabled restrictions, similar artists, the `rotation` and `station` settings.
- The history is what played on decks A and B as far back as separation looks (for daypart separation, back to the start of the daypart it reaches; for the same daypart on the following days, back to that daypart X days before), and when each track last played; the waiting tracks are added at their **planned start**: now, plus what the decks still have to play, plus the waiting tracks' lengths (crossfades not counted).
- The station's [play ratio](rotation.md#play-ratio) counters are loaded with the library and saved after each pick: the picked track's, and those of the tracks its ratio held back.
- The clock carries on from the last pick made from a clock. Each pick goes to the **rotation log** with its explanation and the engine's queue entry; when the entry starts, its play is linked to the pick and carries its relaxation step. Relaxed picks are logged at `info`.
- When restrictions leave nothing to pick, autofill waits 30 seconds before trying again (any change to rotation wakes it): the engine's fallback directory plays meanwhile.
- **Simulations** (`POST /rotation/simulate`) run on the same station, from now (or `start_unix_ms`) after the real history, without touching the queue or the ratio counters (copies of them move in memory), and are audited: `violations` must be empty, `steps` counts the picks per relaxation step. `clock_id` runs one clock at every hour.

The **artist linker** runs every 10 seconds: tracks whose artist or title tag changed are linked to artists again ([Rotation – Artists with several names](rotation.md#artists-with-several-names)), names made of other artists' names become suggestions, and artists nothing refers to any more are forgotten. `POST /artist-suggestions/musicbrainz` queues an `artist_relations` [library task](#library-management): `heron-librarian` looks up every artist with a MusicBrainz ID (one request a second) and suggests the links between the station's artists.

## Library management

The API cannot write files (its library mount is read-only), so what changes files is queued as a **library task** (`library_tasks`) that `heron-librarian` runs, one at a time, in order. The routes answer `202` with the task; poll `GET /library/tasks/{id}` until its `status` is `done` (with its `plan` or `report`) or `failed` (with its `error`). A task stays `pending` while the librarian is stopped.

- **Covers:** see [Cover art](#cover-art).
- **Review:** `GET /library/review` lists the tracks whose identification was not confident enough, with the releases AcoustID found (`candidates`). Applying one queues a `review_apply` task: the librarian looks the release up on MusicBrainz, writes its tags into the file, files it where the layout says and saves the cover, as a `review` batch. Keeping the file's own tags only changes the database (status `kept`).
- **Organize:** `POST /library/organize` queues a dry run; its `plan` lists every move. `POST /library/organize/{plan_task_id}/apply` carries it out as one batch (once per dry run; tracks moved since are skipped).
- **Undo:** `POST /library/batches/{id}/undo` reverts any batch (import, organization, review, tag edit, deletion; admins). Undoing a review puts the track back in review.
- **Tag edits:** `POST /tracks/tags` queues an `edit_tags` task, see [below](#actions-on-a-selection).
- **MusicBrainz details:** `POST /library/musicbrainz-details` queues a `musicbrainz_details` task: the librarian looks up the release of every identified track never looked up (`"all": true`: every one again), one release a second, and saves album, record label, catalog number, ISRC, first release year and genres. Once one has finished, new tracks are looked up in the background.

- **Deletion:** `POST /tracks/delete` queues a `delete` task (operators): the tracks' files go to the trash, as a `delete` batch. `GET /library/trash` lists the deleted tracks with their deletion batch and when they are purged; `POST /library/trash/{batch_id}/restore` undoes a deletion (operators); `POST /library/trash/empty` purges everything now (`empty_trash`, admins; the task's `purge` tells what went). Deleted tracks leave `GET /tracks`; `GET /tracks/{id}` still answers, with `deleted_at_unix_ms`, until they are purged.
Tasks running when the librarian stops are marked failed; ask again (an interrupted undo resumes). See [Library management](library.md).

## Buffer zone

Uploads go into the buffer zone (`HERON_STAGING_DIR`, read-write for the API), never into the library. Operators and admins:

1. `POST /staging/groups` starts an upload: its files are identified together.
2. `POST /staging/uploads` announces a file; it is refused (`422`) when its extension is not an audio one, when it is larger than `max_upload_mb`, or when it would leave less than `min_free_mb` on the disk (counting the uploads in progress).
3. `PUT /staging/uploads/{id}?offset=n` appends a chunk of at most 16 MiB at byte `n`, which must be what the server has, or the answer is `409`. After a dropped connection, `GET /staging/uploads/{id}` tells `received`: carry on from there. With the last byte, the file takes its name and `heron-librarian` identifies it.
4. `GET /staging` lists the groups and their files: status (receiving, identifying, ready, importing, failed), own tags and the tags they get, planned path, identification and candidates, flags, choice, and what they were prepared with (categories, labels, mood, sound code).
5. `POST /staging/files/{id}/choice` picks a candidate release, or keeps the file's own tags (the file is `identifying` again while the librarian works its tags and path out). `POST /staging/import` queues a `staging_import` task (files that are not ready are skipped, see its `report`); `POST /staging/discard` removes files from the buffer zone at once.
6. `POST /staging/files/tags` edits the tags files are imported with: `set` gives fields (`title`, `artist`, `album`, `album_artist`, `year`, `track`, `disc`) a value, or removes them (`null`); `reset` drops edits. Edits go on top of the identification, a picked release or the own tags (`tag_edits` in `GET /staging`); ready files are `identifying` again while the librarian works their path out, without looking the release up. Files being imported are refused (`409`).
7. `POST /staging/files/preparation` prepares files for their tracks: categories added (`[{"category_id", "weight"}]`, weight 1 by default) or removed, labels added or removed, and `mood` and `sound_code` set (a value of the station's scales) or cleared (`null`; left out: kept). Files being imported or no longer waiting are refused (`409`). The track of an import gets them when it is created; `GET /tracks?batch_id=` (the `batch_id` of the task's `report`) lists the tracks an import created.

## Actions on a selection

The routes acting on several tracks take a `selection`: `{"track_ids": [...]}`, or `{"filters": {...}}` with the filters of `GET /tracks` (every matching track, on every page; at most 100,000). Operators and admins.

- `POST /tracks/labels` adds and removes labels; `POST /tracks/categories` puts tracks in categories (filling a category from a label: `{"filters": {"labels_any": "3"}}`) or takes them out; `POST /tracks/programming` sets the mood and sound code (values of the scales in the `programming` settings), the tempo, which replaces the measured one (`null` goes back to it), and the `play_ratio` (tenths, 1 to 10; 0 disables the tracks and keeps their ratio; with `station_id`, the ratio on that station: 0 disables them there). They change the database only and answer how many tracks were selected and how many changes were made.
- `POST /tracks/enabled` (`{"selection", "enabled": false}`) disables the selected tracks, or enables them again: a disabled track does not play at all, as when disabled from its page or given a ratio of 0 (rotation, clocks and medleys never pick it, it cannot be queued by hand, and the queue manager takes it out of the queue). Enabling takes an optional `play_ratio` (tenths, 1 to 10) given to every selected track, and `reset_counters` to put their counters back to 10. With `station_id`, it disables or enables them on that station only, its ratio and counters with them. It answers how many tracks were selected and how many changed.
- `POST /tracks/tags` writes tags into the files (at most 10,000): `set` gives fields the same value on every track (`title`, `artist`, `album`, `album_artist`, `year`, `genre`, `track`, `disc`, `isrc`, `record_label`, `bpm`, `mood`; `null` removes the field), `from_details` writes each track's own ISRC, record label, BPM or mood. It answers `202` with an `edit_tags` library task; its report counts the files written, those already right, and those skipped (missing, changed since the last scan). Refused with `409` while the library setting `write_tags` is off. Fields written drop the track's overrides of the same fields.
- `POST /tracks/tag-overrides`, while `write_tags` is off (`409` otherwise): keeps tags in Heron's database instead of the files (`track_tag_overrides`). `set` as above, tags only (`title`, `artist`, `album`, `album_artist`, `year`, `genre`, `track`, `disc`). Answers `200` with the tracks selected and the overrides changed; the files are not touched. The track list, rotation, history and the stream's metadata show the values at once, and the scanner keeps them when it reads a file again.
- `POST /tracks/tag-overrides/clear` (at most 10,000 tracks): drops the overrides of `fields` (every field when empty or left out), and reads the files' tags back into the tracks. Works whatever `write_tags` says. A track's overrides are listed in `tag_overrides` of `GET /tracks/{id}`.

## Play ratios per station

A track has a default [play ratio](rotation.md#play-ratio), and a station can have its own (`track_station_ratios`).

- `station_ratios` of `GET /tracks/{id}`, one per station in the stations' order, and `station_ratio` of each track of `GET /tracks?on_station_id=`: `station_id`; `play_ratio`, the station's own (tenths, 1 to 10, kept while the track is disabled there), `null` when it follows the default; `enabled`, `false` when it is disabled on that station; `ratio`, what rotation uses there (0 when disabled there or everywhere); `counter` (the track's page only); `disabled_by` and `disabled_at_unix_ms` when disabled there.
- Setting a station's ratio, disabling or enabling a track there, or resetting its counter there takes **operator** (or admin) on that station; the default ratio and the global *Disable* take the shared role, as before. `POST /rotation/counters/reset` takes operator on its station and answers how many counters went back to 10.
- A track disabled on a station is refused when queued by hand there (`422`, *disabled on this station*), and the station's queue manager takes it out of its queue.
- `POST /tracks/ratios/import` (`Content-Type: text/csv`, up to 8 MiB; the web container's nginx lets this route through up to that size): the first line names the columns, separated by commas or semicolons, values in double quotes when they hold one: `path` (the library path) or `filename` (the file name alone, ignoring case), then a station code per column and optionally `default`. A ratio is `0` to `1` in steps of 0.1 (`0.2` or `0,2`), 0 disables; an empty cell changes nothing. The answer has the rows matched, the changes, `unmatched` (each row's line, what named it, and why: `not_found` or `several`), `invalid` (line, column, value) and `unknown_columns`. With `dry_run=true` nothing is written; otherwise the valid values are written in one transaction. Operators of every station named in the columns (and the shared role for `default`).

## Track details

`GET /tracks` and `GET /tracks/{id}` give, besides the tags: the date added (when the scanner first found the file), the play count and last played, the codec, bitrate and bitrate mode (`cbr`, `vbr`, `lossless`), the year of first release (MusicBrainz, else the year tag) and its era, the tempo (set by hand, else measured), mood, sound code and labels. A track's page adds its sample rate, channels, size, MusicBrainz details and the measured tempo.

Plays count from `play_history` without the FX player and without plays shorter than `min_play_s` of the **programming settings** (30 s by default, 0 counts every start); a play still going, or whose end was missed, counts once it started that long ago. The same settings hold the eras (decades when empty: `70s`, `80s`, `2000s`; or the station's, by years of first release, the first matching one applies) and the mood and sound code scales (by default an energy from 1 to 5, and soft, medium, hard). Tracks keep their mood and sound code when a scale changes; sorting puts values off the scale last.

## Plays by day and hour

`GET /tracks/{id}/plays/grid` says when a track played, for the two grids of its page ([Web UI](web.md#plays-by-day-and-hour)). It reads `play_history`, with the plays that count (above), and stores nothing.

- `station_id`: that station's plays; left out, every station's, added up. Times are in the station's time zone, or the first station's with every station.
- `source`: only the plays of that source (`rotation`, `request`, `manual`, `fallback`); left out, all of them.
- `days`: the period of the hours, 1 to 365 days ending today (30 by default).

The answer has the `time_zone`, `today`, and:

- `days`: the 30 days ending today, oldest first: each day's `date`, `weekday` (1 = Monday), its start and end (`start_unix_ms`, `end_unix_ms`, for the history) and its `plays`, in order.
- `hours`: the hours of the week the track played in over `days` days (`hours_from` is the first): each cell's `weekday`, `hour` (0 to 23) and `plays`, the latest first. Cells without plays are left out.

Each play has its `started_at_unix_ms`, its local `date` and `time` (`08:12`), its `station_id` and its `source`. A play goes in the hour its local time shows: when the clocks go back, both 02:00 hours go in the 02:00 cell; when they go forward, that day has no 02:00.

## Play history

Recorded in `play_history` from the engine's track changes: a row when a track starts (track, artist, title, deck, source), completed when it ends (`played_ms`, `end_reason`). FX plays are recorded too (deck `fx`).

While the API is down or cannot reach the engine, the engine plays on and those plays are not recorded. When the API reconnects, the tracks still playing are matched with their rows (or recorded, starting when their position says), and the other open rows are closed at that time, without an end reason.

## Deck EQ

The [deck EQ](audio-engine.md#deck-eq) of the music (Decks A and B) and of the FX player, operators only.

- `GET /settings/eq` gives both EQs; `PUT /settings/eq` replaces them (`{"music": {"enabled", "gains_db"}, "fx": {…}}`, ten gains from −12 to +12 dB, from 60 Hz up; anything else is refused with `422`). The API saves them, pushes them to the engine at once and sends the `eq` live message, so every open Deck EQ page follows. The web UI sends a dragged slider at most every 100 ms; the engine glides between the values it receives.
- Presets: `GET /eq-presets`; `POST /eq-presets` (`{"name", "gains_db"}`, the name unique); `PUT /eq-presets/{id}` (rename, or new gains); `DELETE /eq-presets/{id}`. Loading a preset is the web UI copying its gains into an EQ with `PUT /settings/eq`: presets are never sent to the engine.
- When the API first reaches an engine that has EQ settings and the database has none (set with `ctl set-eq` before the API ran), it adopts the engine's, as it does for the playout settings.

## AGC

The [AGC chain](audio-engine.md#master-agc-chain) of a station, operators only.

- `GET /settings/agc` gives the whole chain; `PUT /settings/agc` replaces it: `bypass_all`, and each block with `enabled` and its settings ([Audio engine – Chain settings](audio-engine.md#chain-settings)). A value out of its range, crossovers not rising or too close, a band or link too many or missing, or an unknown mode is refused with `422`. The API saves the chain, pushes it to the engine at once and sends the `agc` live message, so every open AGC page follows. The web UI sends a dragged control at most every 100 ms; the engine glides between the values it receives.
- Presets, shared by the stations: `GET /agc-presets`; `POST /agc-presets` (`{"name", "settings"}`, the name unique, `settings` the chain without `bypass_all`); `PUT /agc-presets/{id}` (rename, or new settings); `DELETE /agc-presets/{id}`. Loading a preset is the web UI copying its settings into the chain with `PUT /settings/agc`, keeping `bypass_all`: presets are never sent to the engine.
- When the API reaches an engine that has AGC settings and the database has none, it adopts the engine's, as it does for the playout settings and the EQ: set with `ctl set-agc` before the API ran, or on a station the API already managed before M10.

## Engine configuration

Playout settings, the deck EQ, the AGC chain, encoder profiles and outputs are stored in MariaDB and edited through the API. The API pushes them to the engine (`SetPlayoutSettings`, `SetEqSettings`, `SetAgcSettings`, `SetOutputs`) when they change, when the engine starts or becomes reachable again, and checks once a minute; `GET /engine` says whether the engine runs the stored configuration (`synced`, `pending` while it is unreachable, `failed` with the engine's error). Encoder profiles and outputs are checked as a whole configuration before they are saved, the way the engine does: an invalid change saves nothing.

Output settings never show their password, nor the stream server's admin password of their statistics settings; an update that leaves one out keeps the stored one. The statistics settings are not pushed to the engine.

**Upgrading from M2:** the first time the API reaches the engine, it adopts the engine's configuration instead of replacing it: playout settings never saved through the API, and outputs when the database has none, are copied from the engine with their IDs, so the station stays on air. From then on the database is the reference, and changes made with `heron-engine ctl set-outputs` or `set-settings` are replaced within a minute: use the API.

## Web client

`web/openapi.json` is the OpenAPI document, and `web/src/api/schema.ts` the TypeScript types generated from it; `web/src/api/index.ts` is a typed client (`openapi-fetch`). After changing the API:

```sh
cargo run -p heron-api -- openapi > web/openapi.json
cd web && npm run generate-api
```

CI fails when either file is out of date. The generator (`openapi-typescript`) lives in `web/tools/openapi` with its own lockfile: it needs the TypeScript 5 compiler API, which the app's TypeScript 7 no longer has.

## Metrics

On top of the scanner's: `heron_api_uploads_total`, `heron_api_engine_connected` (0 or 1), `heron_api_engine_disconnections_total`, `heron_api_autofill_tracks_total`, `heron_api_rotation_relaxed_total`, `heron_api_logins_total{result}`, `heron_api_live_clients` `heron_api_webhook_deliveries_total{result}` (`delivered`, `retrying`, `failed`), `heron_listeners{output}` and `heron_api_listener_polls_total{result}` ([Listener statistics](statistics.md#metrics)).

## Limitations

- Planned starts ignore crossfades, and a skip or a track queued by hand moves the tracks after it: their dayparting and clock slot were decided for the planned time.
- The clock follows its slots in order; the top of the hour is not hit exactly (no hard sync).
- Plays made while the API is away are not recorded (the engine keeps no event history), and webhooks are not told about them.
- Deliveries to one webhook may arrive out of order when one is retried or several are sent at once: order them by `at_unix_ms`.
- HTTPS is left to a reverse proxy in front of `heron-web`.
