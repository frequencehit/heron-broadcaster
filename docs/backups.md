# Backups

What makes a station hard to rebuild is what people set by hand: its stations, clocks and schedules, categories and restrictions, outputs and their passwords, settings, webhooks, users, and on each track its categories, cue points and markers. The music files rebuild the library (scan and analysis); none of that comes back with them. `heron-api` backs it up every day to a folder, keeps the last days, and restores any backup from the web UI or the command line. Built in [M12](roadmap/m12-config-backups.md).

Each backup is **two files** taken at the same time:

| File | What it is for |
|---|---|
| `heron-config-<time>-<kind>.json` | Everything set by hand, readable, restored from the web UI or the command line on top of a library scanned again ([What it holds](#what-the-configuration-file-holds)) |
| `heron-database-<time>-<kind>.sql.gz` | A dump of the whole database, history and listener statistics included, restored with MariaDB's own tools ([Restoring the database](#restoring-the-database-dump)) |

The configuration file is what a station restores after a bad change or a lost database: it fits any Heron of its version or newer, and leaves the library's own data (analysis, waveforms, fingerprints) to the scan. The database dump is the last resort: everything as it was, history included, but only into the same version of Heron.

## What the configuration file holds

| Saved | Left out |
|---|---|
| Stations (name, code, enabled, order) and every settings group, shared and per station | Tracks themselves, their analysis, waveforms, fingerprints and identification: the scan rebuilds them |
| Encoder profiles and outputs, with their passwords and statistics credentials | The play history, the selection log, listener statistics: in the database dump |
| EQ presets | Library tasks, import batches, the trash journal |
| Category types, categories and their stations | The buffer zone |
| Clocks and their slots, every station's schedules, their periods and hours | Webhook deliveries |
| Restrictions and their stations | Sessions (everyone signs in again after a restore) |
| Labels | Rendered medley files: they are in the library, under `medleys/` |
| Artists curated by hand: marked similar, suggestions accepted or dismissed | Artist names moved or merged by hand: to do again after a restore onto a rebuilt library, or restore the database dump |
| Medleys: recipes, stations and parts | |
| Webhooks, with their secrets | |
| Users (password hashes, shared roles, roles per station, preferences) and API tokens (hashes) | |
| **What is set on each track:** enabled, categories and weights, labels, overrides (cue points, crossfade, where the next track starts), intro and hook markers, BPM, mood and sound code, tags kept in Heron | |

**Secrets are in both files.** Output and stream server passwords, webhook secrets, the AcoustID key, password and token hashes: a restore gives back a working station, with nothing to type again. The files are written readable by the API's user only (`0600`). Treat them like the database: do not mail them, and keep copies off the machine somewhere private.

**Tracks and artists are referred to by what does not change.** A track is named by its library path, an artist by its name: the scan gives them other IDs. On a restore they are matched with the database: whatever names a track or artist that is not found (a track's settings, a restriction, a medley's part) is left out and listed in the restore's report. What the file holds whole (stations, categories, clocks, users…) keeps its IDs, so that what refers to it, in the file and in the settings, still does.

## The configuration file

```json
{
  "format": 1,
  "heron_version": "0.12.0",
  "schema": 20261201000000,
  "created_at": "2026-11-03T03:30:00Z",
  "kind": "daily",
  "stations": […],
  "settings": {"library": {…}, …},
  "tracks": [{"path": "Artist/Album/01 Title.flac", "categories": [{"category": "Gold", "weight": 1.0}], …}],
  …
}
```

- `format` is the version of this document's layout; `schema`, the last database migration of the Heron that wrote it.
- `kind`: `daily`, `manual` (backed up on request), `before_restore` (taken before a restore), `uploaded`. It is in the files' names too: a backup's name is its time and kind, `2026-11-03T03-30-00Z-daily`.
- A Heron restores files of its own format and older ones. A file written by a newer Heron is refused, with its version in the message.

## Where and when

- **Folder:** `HERON_BACKUP_DIR` in `heron-api` (`/var/lib/heron/backups` in the image). `docker-compose.yml` mounts a folder of the host there, `./data/backups` unless `HERON_BACKUP_FOLDER` says another: **to keep copies off the machine, point it at a folder that is**, a network share or a folder another tool synchronizes. Only `heron-api` writes there, as `HERON_LIBRARIAN_USER`: create the folder before starting Heron (`mkdir -p data/backups`), owned by that user, as for the buffer zone.
- **Daily backup:** at 03:30 in the first station's time zone (`backups.time`). A day the API is down is caught up when it starts, if the last daily backup is more than a day old.
- **Kept:** the last 7 daily backups (`backups.keep_days`, 1 to 90), the backups taken before the last 7 restores, and every manual and uploaded backup until it is deleted. Older daily backups are deleted after a successful one, never before. A backup's two files go together.
- **Manual backup:** *Back up now* on the Backups page, `POST /backups`, or `heron-api backup create`.
- **The database dump** is taken with `mariadb-dump --single-transaction` (the station plays on), compressed, for every backup but `uploaded` ones; `backups.database` turns it off. A failed dump is reported on the Backups page and in the logs, and does not stop the configuration file.
- Each file is written to a temporary file, synced, then renamed: a crash never leaves half a file under a backup's name.

## Restoring the configuration

Admins only. From the **Backups** page (pick a backup, or upload a configuration file), from the API (`POST /backups/{name}/restore`, `POST /backups/restore` with a file), or on the command line:

```sh
docker compose exec heron-api heron-api backup list
docker compose exec heron-api heron-api backup restore 2026-11-03T03-30-00Z-daily
docker compose restart heron-api    # the engines get the restored configuration
```

1. The file is checked first: its format, its schema, every part of it. A file that does not pass changes nothing.
2. The current configuration is backed up (`before_restore`), so a restore can itself be undone.
3. Everything the file holds replaces what the database has, in one transaction: a restore applies whole or not at all. Rows are matched by ID: what the database has and the file does not (a category made since) is deleted, except stations, which are left as they are with their history. What is set on tracks is replaced for every track: a track the file does not name (added since) loses its categories and settings. What the file does not hold (the tracks themselves, history…) is not touched.
4. Every session is closed: sign in again, with the users of the backup. When no admin can sign in afterwards, `heron-api user passwd` sets a password from the command line.
5. Each engine gets its station's restored playout settings, deck EQ and outputs at once, as after any change ([API – Engine configuration](api.md#engine-configuration)); outputs whose settings did not change keep streaming.

The restore's report lists what it replaced, counted by kind, and what it left out: the tracks it did not find by path, with how many settings each had, and the restrictions and medley parts whose track or artist is gone.

**After losing the database:** start Heron with an empty database, let it scan and analyze the library, then restore the last configuration file: the stations, their programming and every track's settings come back on top of the rebuilt library. Only the history and listener statistics are missing: restore the database dump instead to get them back.

## Restoring the database dump

The dump replaces the whole database with what it was at the time of the backup, history included. It is restored with MariaDB's tools, Heron stopped but for its database, into the **same version** of Heron (migrations run forward from there when Heron starts):

```sh
docker compose stop heron-api heron-librarian heron-analyzer
gunzip -c data/backups/heron-database-2026-11-03T03-30-00Z-daily.sql.gz \
  | docker compose exec -T mariadb sh -c 'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" heron'
docker compose start heron-api heron-librarian heron-analyzer
```

The engines play on throughout, with what they have queued.

## Downloading and uploading

- `GET /backups` lists the backups (name, kind, time, the size of each file, whether the dump failed); `GET /backups/{name}/config` and `GET /backups/{name}/database` download its files; `DELETE /backups/{name}` deletes both. Admins only: the files hold secrets.
- Uploading a configuration file restores it; it is kept as an `uploaded` backup.

## Settings

`backups` in the `settings` table:

| Setting | Default |
|---|---|
| `enabled` | true |
| `time` | `03:30`, in the first station's time zone |
| `keep_days` | 7 |
| `database` | true: a database dump with each backup |
