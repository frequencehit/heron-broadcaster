# API and control plane

`heron-api` is the station's control plane: the REST API and live WebSocket the [web UI](web.md) uses, the queue manager feeding the engine, the play history, and the configuration it pushes to the engine. Built in [M4](roadmap/m4-api-control-plane.md), completed for the web UI in [M5](roadmap/m5-web-ui.md); rotation came with [M6](roadmap/m6-rotation-clockwheels.md), webhooks and the public endpoints with [M7](roadmap/m7-webhooks-output-api.md).

The engine never depends on it: when the API is down, the engine plays what it has queued with the configuration it last received ([Audio engine](audio-engine.md)).

## Basics

- Every route is under `/api/v1`. `heron-web` proxies `/api` to `heron-api`, so the API is at `http://<host>:8000/api/v1`.
- Requests and responses are JSON. Times are Unix time in milliseconds (`*_unix_ms`), durations in milliseconds (`*_ms`).
- Errors are `{"error": "<code>", "message": "…"}`: `invalid_request` (400), `unauthorized` (401), `forbidden` (403), `not_found` (404), `conflict` (409), `invalid` (422), `too_many_requests` (429), `engine_unavailable` (503), `internal` (500).
- **OpenAPI:** the document is generated from the code and served at `/api/v1/openapi.json`; `heron-api openapi` prints it. It is the reference for every route and type below.

## Authentication

Local users only for now ([Decisions](decisions.md#decisions-made-after-the-handoff)).

| Role | Can |
|---|---|
| `viewer` | See everything except users and secrets (output passwords, the AcoustID key are never sent back to anyone) |
| `operator` | Also run the station: queue, skip, pause, FX, track overrides and categories, queue and rotation settings, clocks, the schedule, restrictions, artists, simulations, the review of identifications |
| `admin` | Also playout, analysis, library and station (time zone) settings, organizing the library and undoing batches, encoder profiles, outputs, webhooks and users |

Create the first admin on the command line (the password is read without echo; from stdin when it is not a terminal):

```sh
docker compose exec heron-api heron-api user add admin --role admin
docker compose exec heron-api heron-api user passwd admin
docker compose exec heron-api heron-api user list
```

Then:

- **Web app:** `POST /api/v1/auth/login` with `{"username", "password"}` sets a session cookie (`heron_session`, `HttpOnly`, `SameSite=Strict`, `Secure` when the proxy reports HTTPS through `X-Forwarded-Proto`). Sessions last 30 days from the last use. Requests that change something with a cookie must also send an `X-Requested-With` header (any value): pages on other sites cannot add it, so they cannot act on your behalf.
- **Scripts:** create an API token (`POST /api/v1/tokens`, shown once) and send `Authorization: Bearer heron_…`. Tokens act as their user, with its role; they can expire and be revoked.
- Passwords (8 characters at least) are hashed with Argon2id, one at a time so sign-ins never take CPU from the engine on a Raspberry Pi. After 5 failed sign-ins a username is locked for a minute. Only hashes of session and API tokens are stored. Changing a password signs the user out everywhere else; disabling a user ends their sessions and stops their tokens. The last enabled admin cannot be demoted, disabled or deleted.
- `GET /api/v1/status`, `/api/v1/library/status`, `/api/v1/openapi.json` and the [public endpoints](#public-endpoints) (`/api/v1/public/…`) need no sign-in. `/status` also reports the server clock's synchronization (`clock`, read from the kernel with a read-only `adjtimex`: containers share the host's clock), with a `warning` when it is not synchronized or may be off by more than a second; the web UI shows it on every page.

## Routes

| Area | Routes |
|---|---|
| Account | `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`, `PUT /auth/password`, `PUT /auth/preferences` (`{"theme", "language", "full_width"}`, each optional: the theme is system, light or dark; the language is system or a language tag such as `fr`; `full_width` drops the centred column); `GET`, `POST /tokens`, `DELETE /tokens/{id}` |
| Users (admins) | `GET`, `POST /users`; `PATCH`, `DELETE /users/{id}` |
| Engine and decks | `GET /engine` (reachable, configuration in sync), `GET /player` (decks, cued track, queue, outputs), `GET /player/levels`, `POST /player/skip`, `/player/pause`, `/player/resume`, `POST /player/fx` (`{"track_id"}`), `DELETE /player/fx` |
| Queue | `GET /queue` (the cued track first), `POST /queue` (`{"track_ids", "position"}`), `DELETE /queue`, `DELETE /queue/{entry_id}`, `PUT /queue/{entry_id}/position` |
| Library | `GET /library/status`; `GET /tracks` (`q`, `category_id`, `filter`: playable, missing, unanalyzed, silent, disabled, needs_review; `labels_all`, `labels_any`, `labels_none` (comma-separated IDs); `never_played`, `not_played_days`, `added_days`, `batch_id` (tracks an import created), `bitrate_below`, `era`, `mood`, `sound_code`, `genre`, `year_min`, `year_max`, `bpm_min`, `bpm_max`; `sort`: artist, title, album, genre, added, last_played, play_count, bitrate, duration, year, bpm, mood, sound_code, and `desc`; `limit`, `offset`); `GET`, `PATCH /tracks/{id}` (`enabled`); `PUT`, `DELETE /tracks/{id}/overrides`; `PUT /tracks/{id}/categories`; `GET /tracks/{id}/waveform`; `GET /tracks/{id}/audio` (the file, with range requests, for previews); `POST /playout/transition` (preview a transition between two tracks, optionally with unsaved settings) |
| Library management | `POST /library/musicbrainz-details` (`{"all"}`); `GET /library/review`; `POST /tracks/{id}/review/apply` (`{"release_id", "recording_id"}`, one of its candidates), `POST /tracks/{id}/review/keep`; `POST /library/organize` (dry run), `POST /library/organize/{plan_task_id}/apply`; `GET /library/batches`, `POST /library/batches/{id}/undo`; `GET /library/tasks`, `GET /library/tasks/{id}`; `POST /tracks/delete` (`{"track_ids"}`); `GET /library/trash`, `POST /library/trash/{batch_id}/restore`, `POST /library/trash/empty` — see [below](#library-management) |
| Import | `POST /library/import/preview` (`{"rows"}`), `POST /library/import/apply` (`{"categories", "problems", "rows"}`), `GET /library/imports`, `POST /library/imports/{id}/undo`: admins, see [Importing from another software](import.md) |
| Buffer zone | `GET /staging`; `POST /staging/groups` (`{"name"}`); `POST /staging/uploads` (`{"group_id", "name", "size"}`), `GET /staging/uploads/{id}`, `PUT /staging/uploads/{id}?offset=` (a chunk, `application/octet-stream`); `POST /staging/files/{id}/choice` (`{"kind": "release", "release_id", "recording_id"}` or `{"kind": "own"}`); `POST /staging/files/tags` (`{"file_ids", "set", "reset"}`); `POST /staging/files/preparation` (`{"file_ids", "add_categories", "remove_categories", "add_labels", "remove_labels", "mood", "sound_code"}`); `GET /staging/files/{id}/audio`; `POST /staging/import`, `POST /staging/discard` (`{"file_ids"}`) — see [below](#buffer-zone) |
| Categories | `GET`, `POST /categories`; `GET`, `PUT`, `DELETE /categories/{id}` (with its `type_id` and the separation windows it overrides); `GET`, `POST /category-types`, `PUT`, `DELETE /category-types/{id}` (`{"name", "no_two_in_a_row", "sends_metadata"}`, see [Outputs – Metadata](outputs.md#metadata) and see [Never two in a row](rotation.md#never-two-in-a-row); a type in use cannot be deleted, `409`) |
| Labels | `GET`, `POST /labels`; `PUT`, `DELETE /labels/{id}`; `POST /labels/{id}/merge` (`{"into_id"}`) |
| Selections | `POST /tracks/labels` (`add`, `remove`), `POST /tracks/categories` (`add` with weights, `remove`), `POST /tracks/programming` (`mood`, `sound_code`, `bpm`; `null` clears), `POST /tracks/tags` (a library task), `POST /tracks/tag-overrides` and `POST /tracks/tag-overrides/clear` (tags kept in Heron's database) — see [below](#actions-on-a-selection) |
| Artists | `GET /artists` (`q` matches any name, `limit`, `offset`); `GET`, `PATCH /artists/{id}` (`name`, `musicbrainz_id`); `POST /artists/{id}/merge` (`{"artist_id"}`, merged into this one); `PUT /artist-names/{id}` (`{"artist_id"}` moves the name, `{}` splits it off); `PUT`, `DELETE /artists/{id}/similar/{other}`; `GET /artist-suggestions`, `POST /artist-suggestions/{id}` (`{"accept"}`); `POST /artist-suggestions/musicbrainz` (a library task) |
| Rotation | `GET`, `POST /clocks`; `GET`, `PUT`, `DELETE /clocks/{id}` (name, `looping` and slots, replaced as a whole); `POST /clocks/preview` (`{"slots", "looping"}`, saved or not: expected starts, categories running short); `GET`, `PUT /schedule` (the whole week); `GET /restrictions` (`target`, `target_id`), `POST /restrictions`, `PUT`, `DELETE /restrictions/{id}`; `GET /rotation/picks` (the selection log: `before`, `track_id`, `limit`); `POST /rotation/simulate` (`hours`, `start_unix_ms`, `seed`, `clock_id`) |
| History | `GET /history` (`before`, `after`, `track_id`, `exclude_fx`, `limit`) |
| Settings | `GET`, `PUT /settings/playout`, `/settings/analysis`, `/settings/queue`, `/settings/rotation`, `/settings/station`, `/settings/programming`, `/settings/statistics`; `GET`, `PATCH /settings/library` |
| Outputs | `GET`, `POST /encoder-profiles`; `GET`, `PUT`, `DELETE /encoder-profiles/{id}`; `GET`, `POST /outputs`; `GET`, `PUT`, `DELETE /outputs/{id}` (with each output's live state, and its `statistics` settings: the stream server's admin credentials) |
| Webhooks (admins) | `GET`, `POST /webhooks`; `GET`, `PUT`, `DELETE /webhooks/{id}`; `POST /webhooks/{id}/secret`; `POST /webhooks/{id}/test`; `GET /webhooks/{id}/deliveries` (`before`, `status`, `limit`); `POST /webhook-deliveries/{id}/retry` — see [below](#webhooks) |
| Listener statistics | `GET /listeners/now`, `/listeners/samples`, `/listeners/days`, `/listeners/hours-of-day`, `/listeners/tracks`, `/listeners/players` (CSV with `format=csv` for days, hours and tracks); `GET`, `PUT /settings/statistics` (admins) — see [Listener statistics](statistics.md#api) |
| Public | `GET /public/now-playing`, `GET /public/history` (`before`, `limit`) — no sign-in, see [below](#public-endpoints) |
| Live | `GET /live` (WebSocket) |

Every engine action is there: queue (enqueue, remove, clear, move, and replace, which the queue manager uses), transport (skip, pause, resume), FX player, state, levels, track changes (WebSocket), playout settings and outputs. Tracks the engine plays from its fallback directory show with the source `fallback`; autofill does not count them as queued, and anything queued replaces a cued one.

## Live WebSocket

`/api/v1/live`, signed in like any request (the browser sends the session cookie). Each message is a `LiveMessage` (see the OpenAPI document), told apart by `type`:

| `type` | When | Content |
|---|---|---|
| `state` | On connection, then whenever the decks, the queue, the outputs or the engine's reachability change | `player`: the same as `GET /player`, `null` while the engine is unreachable |
| `positions` | Twice a second | Each deck's position and length |
| `levels` | Ten times a second, unless the URL has `?levels=false` | Peak and RMS (dBFS) of the master bus and each deck |
| `track` | Each track change | `event`: `started` or `ended` (with `played_ms` and `reason`: finished, skipped, stopped, replaced) |

The API polls the engine's state twice a second and its levels ten times a second, only while someone watches them.

## Webhooks

Webhooks tell other systems (a website, a chat bot, a studio display) about track changes as they happen. Admins manage them; their secrets are never sent back.

- **Events:** `track.started`, `track.ended` and `track.skipped`, as the engine reports them: a skip is `track.skipped` (not also `track.ended`), any other end (played to its transition, an FX stopped or replaced) is `track.ended`. Tracks of the FX player, and tracks that send no metadata (a category type with `sends_metadata` off: jingles, ads), are only sent to webhooks with `include_fx`; tracks from the fallback directory are sent like any other.
- **Body:** by default the JSON `Payload` of the OpenAPI document:

  ```json
  {"event": "track.started", "delivery_id": 42, "at_unix_ms": 1792917865323,
   "track": {"track_id": 17, "artist": "Artist", "title": "Title", "album": "Album",
             "duration_ms": 215000, "deck": "a", "source": "rotation"},
   "played_ms": null, "reason": null}
  ```

  Or a **body template** of your own, with its content type, for receivers that want a format of their own (Discord, Slack, a display). Placeholders: `{{event}}`, `{{delivery_id}}`, `{{at_unix_ms}}`, `{{track_id}}`, `{{artist}}`, `{{title}}`, `{{album}}`, `{{duration_ms}}`, `{{deck}}`, `{{source}}`, `{{played_ms}}`, `{{reason}}` (spaces inside the braces are allowed). When the content type is JSON, values are escaped to fit between quotes: `{"content": "Now playing: {{artist}} – {{title}}"}`. Missing values are empty; unknown placeholders are refused (`422`).
- **Requests:** `POST` to the webhook's URL (`http` or `https`; it may be an address of your network: only admins can set it), with `X-Heron-Event`, `X-Heron-Delivery` (the delivery's ID), `X-Heron-Timestamp` (Unix seconds) and `X-Heron-Signature`. Redirects are not followed.
- **Signature:** `X-Heron-Signature: sha256=<hex>`, the HMAC-SHA256 of `"{timestamp}.{body}"` with the webhook's secret (`whsec_…`, shown once when the webhook is created or given a new secret with `POST /webhooks/{id}/secret`). Receivers should compare it in constant time and refuse old timestamps. In Python:

  ```python
  expected = "sha256=" + hmac.new(secret.encode(), f"{timestamp}.{body}".encode(), hashlib.sha256).hexdigest()
  ok = hmac.compare_digest(expected, request.headers["X-Heron-Signature"])
  ```

- **Delivery and retries:** deliveries are sent as soon as the engine reports the change, at most 8 at a time. A delivery that does not get a `2xx` within 10 seconds is retried after 10 s, 30 s, 2 min, 10 min, 30 min, 1 h and 3 h, then marked `failed` (8 attempts). Deliveries are rows in the database, so retries survive a restart of the API; one being sent when the API stopped is sent again a minute later, so receivers may see a delivery twice (same `X-Heron-Delivery`). Deliveries of a disabled webhook that were still pending fail.
- **Delivery log:** `GET /webhooks/{id}/deliveries` lists what was sent, its status (`pending`, `delivered`, `failed`), attempts, the receiver's status code, the error and the start of its answer (1 KiB). `POST /webhooks/{id}/test` sends a `ping` (even to a disabled webhook); `POST /webhook-deliveries/{id}/retry` sends a delivered or failed delivery again, with every attempt again. Deliveries are kept 30 days.

## Public endpoints

For station websites, apps and displays: no sign-in, readable from any origin (`Access-Control-Allow-Origin: *`, `GET` only, no credentials), the music only (decks A and B, not the FX player, and not tracks that send no metadata: while a jingle is on air, now-playing shows the song before it), without IDs or paths.

- `GET /api/v1/public/now-playing`: `now` (artist, title, album, `started_at_unix_ms`, `duration_ms`, `position_ms`), `null` while nothing plays or the engine is unreachable, and `server_time_unix_ms`. Cached 5 seconds (`Cache-Control: public, max-age=5`).
- `GET /api/v1/public/history`: the latest plays, most recent first (artist, title, album, `started_at_unix_ms`, `ended_at_unix_ms`); `limit` 1 to 50 (20), `before` for the next page. Cached 15 seconds.

They answer through `heron-web` like the rest of the API (`http://<host>:8000/api/v1/public/now-playing`). Put a cache or rate limit in the reverse proxy in front of it if the station is busy.

## Queue manager

The engine holds the queue; the API feeds it and edits it.

- Tracks are sent with everything the engine needs, from the database: path, tags, cue points and crossfade point from the analysis, loudness, and the per-track overrides (cue points, Gap Killer, crossfade). Tracks whose file is missing, or analyzed with nothing audible, are refused. Tracks not analyzed yet play whole.
- **Autofill** (`queue` settings, on by default): when fewer than `min_queued` tracks wait (3), it queues the tracks [rotation](#rotation) picks among the enabled, analyzed, audible tracks. Turned off, the station plays silence once the queue is empty.
- **Positions** count the waiting tracks: 0 is next (before the cued track), 1 right after it, and so on.
- **Following the library:** every 5 seconds, waiting tracks are checked against the database. A track the librarian moved, or whose cue points changed, is updated in place (same entry and position); a track whose file disappeared or was deleted leaves the queue. The track on air plays on (its file is already open).
- Each track carries its source (`rotation` for autofill, `manual` when queued by hand), recorded in the history.

## Rotation

Autofill asks `heron-rotation` for each track it adds ([Rotation](rotation.md)):

- The station is loaded from the database each time: playable tracks, their artists and categories, clocks, the schedule, enabled restrictions, similar artists, the `rotation` and `station` settings.
- The history is what played on decks A and B as far back as separation looks, and when each track last played; the waiting tracks are added at their **planned start**: now, plus what the decks still have to play, plus the waiting tracks' lengths (crossfades not counted).
- The clock carries on from the last pick made from a clock. Each pick goes to the **selection log** with its explanation and the engine's queue entry; when the entry starts, its play is linked to the pick and carries its relaxation step. Relaxed picks are logged at `info`.
- When restrictions leave nothing to pick, autofill waits 30 seconds before trying again (any change to rotation wakes it): the engine's fallback directory plays meanwhile.
- **Simulations** (`POST /rotation/simulate`) run on the same station, from now (or `start_unix_ms`) after the real history, without touching the queue, and are audited: `violations` must be empty, `steps` counts the picks per relaxation step. `clock_id` runs one clock at every hour.

The **artist linker** runs every 10 seconds: tracks whose artist or title tag changed are linked to artists again ([Rotation – Artists with several names](rotation.md#artists-with-several-names)), names made of other artists' names become suggestions, and artists nothing refers to any more are forgotten. `POST /artist-suggestions/musicbrainz` queues an `artist_relations` [library task](#library-management): `heron-librarian` looks up every artist with a MusicBrainz ID (one request a second) and suggests the links between the station's artists.

## Library management

The API cannot write files (its library mount is read-only), so what changes files is queued as a **library task** (`library_tasks`) that `heron-librarian` runs, one at a time, in order. The routes answer `202` with the task; poll `GET /library/tasks/{id}` until its `status` is `done` (with its `plan` or `report`) or `failed` (with its `error`). A task stays `pending` while the librarian is stopped.

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

- `POST /tracks/labels` adds and removes labels; `POST /tracks/categories` puts tracks in categories (filling a category from a label: `{"filters": {"labels_any": "3"}}`) or takes them out; `POST /tracks/programming` sets the mood and sound code (values of the scales in the `programming` settings) and the tempo, which replaces the measured one (`null` goes back to it). They change the database only and answer how many tracks were selected and how many changes were made.
- `POST /tracks/tags` writes tags into the files (at most 10,000): `set` gives fields the same value on every track (`title`, `artist`, `album`, `album_artist`, `year`, `genre`, `track`, `disc`, `isrc`, `record_label`, `bpm`, `mood`; `null` removes the field), `from_details` writes each track's own ISRC, record label, BPM or mood. It answers `202` with an `edit_tags` library task; its report counts the files written, those already right, and those skipped (missing, changed since the last scan). Refused with `409` while the library setting `write_tags` is off. Fields written drop the track's overrides of the same fields.
- `POST /tracks/tag-overrides`, while `write_tags` is off (`409` otherwise): keeps tags in Heron's database instead of the files (`track_tag_overrides`). `set` as above, tags only (`title`, `artist`, `album`, `album_artist`, `year`, `genre`, `track`, `disc`). Answers `200` with the tracks selected and the overrides changed; the files are not touched. The track list, rotation, history and the stream's metadata show the values at once, and the scanner keeps them when it reads a file again.
- `POST /tracks/tag-overrides/clear` (at most 10,000 tracks): drops the overrides of `fields` (every field when empty or left out), and reads the files' tags back into the tracks. Works whatever `write_tags` says. A track's overrides are listed in `tag_overrides` of `GET /tracks/{id}`.

## Track details

`GET /tracks` and `GET /tracks/{id}` give, besides the tags: the date added (when the scanner first found the file), the play count and last played, the codec, bitrate and bitrate mode (`cbr`, `vbr`, `lossless`), the year of first release (MusicBrainz, else the year tag) and its era, the tempo (set by hand, else measured), mood, sound code and labels. A track's page adds its sample rate, channels, size, MusicBrainz details and the measured tempo.

Plays count from `play_history` without the FX player and without plays shorter than `min_play_s` of the **programming settings** (30 s by default, 0 counts every start); a play still going, or whose end was missed, counts once it started that long ago. The same settings hold the eras (decades when empty: `70s`, `80s`, `2000s`; or the station's, by years of first release, the first matching one applies) and the mood and sound code scales (by default an energy from 1 to 5, and soft, medium, hard). Tracks keep their mood and sound code when a scale changes; sorting puts values off the scale last.

## Play history

Recorded in `play_history` from the engine's track changes: a row when a track starts (track, artist, title, deck, source), completed when it ends (`played_ms`, `end_reason`). FX plays are recorded too (deck `fx`).

While the API is down or cannot reach the engine, the engine plays on and those plays are not recorded. When the API reconnects, the tracks still playing are matched with their rows (or recorded, starting when their position says), and the other open rows are closed at that time, without an end reason.

## Engine configuration

Playout settings, encoder profiles and outputs are stored in MariaDB and edited through the API. The API pushes them to the engine (`SetPlayoutSettings`, `SetOutputs`) when they change, when the engine starts or becomes reachable again, and checks once a minute; `GET /engine` says whether the engine runs the stored configuration (`synced`, `pending` while it is unreachable, `failed` with the engine's error). Encoder profiles and outputs are checked as a whole configuration before they are saved, the way the engine does: an invalid change saves nothing.

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
