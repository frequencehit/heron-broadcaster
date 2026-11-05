# Backups

What makes a station hard to rebuild is what people set by hand: its stations, clocks and schedules, categories and restrictions, outputs and their passwords, settings, webhooks, users, and on each track its categories, cue points and markers. The music files rebuild the library (scan and analysis); none of that comes back with them. `heron-api` backs it up every day to a folder, keeps the last days, and restores any backup from the web UI or the command line. Built in [M12](roadmap/m12-config-backups.md).

Each backup is **two files** taken at the same time:

| File | What it is for |
|---|---|
| `heron-config-<time>-<kind>.json` | Everything set by hand, readable, restored from the web UI or the command line on top of a library scanned again ([What it holds](#what-the-configuration-file-holds)) |
| `heron-database-<time>-<kind>.sql.gz` | A dump of the whole database, history and listener statistics included, restored with MariaDB's own tools ([Restoring the database](#restoring-the-database-dump)) |

With [encryption](#encryption) on, both files are encrypted with the station's key and end in `.age`. A third file, `heron-checksums-<time>-<kind>.sha256`, holds the checksums of both as written ([Testing backups](#testing-backups)).

The configuration file is what a station restores after a bad change or a lost database: it fits any Heron of its version or newer, and leaves the library's own data (analysis, waveforms, fingerprints) to the scan. The database dump is the last resort: everything as it was, history included, but only into the same version of Heron.

## What the configuration file holds

| Saved | Left out |
|---|---|
| Stations (name, code, enabled, order) and every settings group, shared and per station | Tracks themselves, their analysis, waveforms, fingerprints and identification: the scan rebuilds them |
| Encoder profiles and outputs, with their passwords and statistics credentials | The play history, the rotation log, listener statistics: in the database dump |
| EQ and AGC presets | Library tasks, import batches, the trash journal |
| Category types, categories and their stations | The buffer zone |
| Clocks and their slots, every station's schedules, their periods and hours | Webhook deliveries |
| Restrictions and their stations | Sessions (everyone signs in again after a restore) |
| Labels | Rendered medley files: they are in the library, under `medleys/` |
| Artists curated by hand: marked similar, suggestions accepted or dismissed | Artist names moved or merged by hand: to do again after a restore onto a rebuilt library, or restore the database dump |
| Medleys: recipes, stations and parts | |
| Webhooks, with their secrets | |
| Users (password hashes, shared roles, roles per station, preferences) and API tokens (hashes) | |
| **What is set on each track:** enabled, categories and weights, labels, overrides (cue points, crossfade, where the next track starts), intro and hook markers, BPM, mood and sound code, tags kept in Heron | |

**Secrets are in both files.** Output and stream server passwords, webhook secrets, the AcoustID key, password and token hashes: a restore gives back a working station, with nothing to type again. The files are written readable by the API's user only (`0600`). In clear, treat them like the database: do not mail them, and keep copies off the machine somewhere private. [Encrypted](#encryption), they can go anywhere: without the key they are unreadable.

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

- **Folder:** `HERON_BACKUP_DIR` in `heron-api` (`/var/lib/heron/backups` in the image). `docker-compose.yml` mounts a folder of the host there, `./data/backups` unless `HERON_BACKUP_FOLDER` says another: **to keep copies off the machine, point it at a folder that is**, a network share or a folder another tool synchronizes. Only `heron-api` writes there, as `HERON_LIBRARIAN_USER`: create the folder before starting Heron (`mkdir -p data/backups`), owned by that user, as for the buffer zone. Docker creates a missing folder owned by root, which the API cannot write: the API checks the folder when it starts and before each backup, and every page shows a banner, for admins, until it can write there: with the command that fixes it (`chown`) when the folder is not the API's user's, with the system's error otherwise (a full disk, a read-only share).
- **Daily backup:** at 03:30 in the first station's time zone (`backups.time`). A day the API is down is caught up when it starts, if the last daily backup is more than a day old.
- **Kept:** the last 7 daily backups (`backups.keep_days`, 1 to 90), the backups taken before the last 7 restores, and every manual and uploaded backup until it is deleted. Older daily backups are deleted after a successful one, never before. A backup's files go together.
- **Manual backup:** *Back up now* on the Backups page, `POST /backups`, or `heron-api backup create`.
- **One at a time:** a backup or restore holds a lock on `.heron-backup.lock` in the folder, so `heron-api backup` on the command line waits for one the API is taking, and the other way round (on a share without file locks, the API only waits for its own, and logs a warning).
- **The database dump** is taken with `mariadb-dump --single-transaction` (the station plays on), over TCP to the host and port of `HERON_DATABASE_URL` (an IPv6 address in brackets too), compressed, for every backup but `uploaded` ones; `backups.database` turns it off. A failed dump is reported on the Backups page and in the logs, and does not stop the configuration file.
- Each file is written to a temporary file, synced, then renamed: a crash never leaves half a file under a backup's name. Each one is checked before it counts ([Testing backups](#testing-backups)).

## Restoring the configuration

Admins only. From the **Backups** page (pick a backup, or upload a configuration file), from the API (`POST /backups/{name}/restore`, `POST /backups/restore` with a file), or on the command line:

```sh
docker compose exec heron-api heron-api backup list
docker compose exec heron-api heron-api backup restore 2026-11-03T03-30-00Z-daily
docker compose restart heron-api    # the engines get the restored configuration
```

An encrypted backup needs its key: from the web UI and the API, it is typed with the restore. On the command line, it is read from a file, or from the standard input with `-`:

```sh
docker compose exec -T heron-api heron-api backup restore 2026-11-03T03-30-00Z-daily --key-file - < heron-backup-key.txt
```

1. The file is checked first: its format, its schema, every part of it. A file that does not pass changes nothing.
2. The current configuration is backed up (`before_restore`), so a restore can itself be undone.
3. Everything the file holds replaces what the database has, in one transaction: a restore applies whole or not at all. Rows are matched by ID: what the database has and the file does not (a category made since) is deleted, except stations, which are left as they are with their history. What is set on tracks is replaced for every track: a track the file does not name (added since) loses its categories and settings. What the file does not hold (the tracks themselves, history…) is not touched.
4. Every session is closed: sign in again, with the users of the backup. When no admin can sign in afterwards, `heron-api user passwd` sets a password from the command line.
5. Each engine gets its station's restored playout settings, deck EQ, AGC chain and outputs at once, as after any change ([API – Engine configuration](api.md#engine-configuration)); outputs whose settings did not change keep streaming.

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

An encrypted dump is decrypted with [age](https://age-encryption.org) (or `rage`), which most systems package, and the key:

```sh
age -d -i heron-backup-key.txt data/backups/heron-database-2026-11-03T03-30-00Z-daily.sql.gz.age \
  | gunzip -c \
  | docker compose exec -T mariadb sh -c 'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" heron'
```

The engines play on throughout, with what they have queued.

## Downloading and uploading

- `GET /backups` lists the backups (name, kind, time, the size of each file, whether the dump failed); `GET /backups/{name}/config` and `GET /backups/{name}/database` download its files; `DELETE /backups/{name}` deletes both. Admins only: the files hold secrets.
- Uploading a configuration file restores it; it is kept as an `uploaded` backup. An encrypted file is kept as it is; a file in clear is encrypted first when encryption is on.
- Encrypted backups download encrypted: Heron cannot decrypt them.

## Encryption

Backups can be encrypted with [age](https://age-encryption.org/v1), a standard and documented file format (X25519 and ChaCha20-Poly1305): a backup cannot be read without the key, and a changed file is refused. Encryption is optional and off until an admin sets a key. Until then, admins see a banner: *backups are not encrypted*. Built in [M15](roadmap/m15-encrypted-backups.md).

**What it protects:** the copies off the machine (a share, a synchronized folder, a download), and backups downloaded with a stolen admin session. **What it does not:** someone with access to the server itself, who can read the database the backups come from.

### The key

**Heron keeps only the public key.** It encrypts every backup with it, daily ones too, with nobody signed in, and cannot decrypt any of them: restoring an encrypted backup asks for the key. The key is a single line, `AGE-SECRET-KEY-1…`, 74 characters: save it in a password manager, and keep it somewhere that does not depend on this server.

**There is no way to recover a lost key.** Backups encrypted with it cannot be read by anyone, Heron included.

On the **Backups** page, *Encrypt backups*:

1. Heron generates a key and shows it once, with its public key, a copy button and `heron-backup-key.txt` to download (an age identity file, which `age -i` reads, with the public key in a comment). It does not keep it, and no page or route shows it again.
2. Type the key back. Encryption is turned on only when it matches: this is the first check, and the proof that the key was saved.
3. Heron encrypts the backups already in the folder: each file is written encrypted, synced, and only then is the file in clear deleted. Copies of them already elsewhere (on a share, downloaded) stay in clear: delete them.
4. Heron takes a backup, so there is always one with the new key.

An admin who already has an age key (made with `age-keygen`) can give its public key, `age1…`, instead of having Heron generate one. Step 1 is then skipped; the key is still typed back.

The key travels to Heron only to be checked or to restore a backup, over the web UI's connection. It is used in memory, then dropped, and never logged.

**Which key to type.** Wherever a key is asked for (typed back, checked, for a restore or a test), the page shows the public key it must match: the pending one, the current one, or the earlier one a backup was encrypted with. Save the public key with the key in the password manager: after a key change, it tells which saved key opens which backup. An uploaded encrypted file has no public key shown, as an age file does not name its key.

### Checking the key

So that a lost key is noticed before a restore needs it, Heron asks each admin, now and then, to type the key back, as messaging apps do with a recovery PIN. A dialog opens on any page:

- **Check:** a correct key also [tests the newest backup](#with-the-key-a-test-read), and moves the next check further away: 1, 3, 7 and 14 days, then every 30 days. A wrong key is only refused, and can be typed again.
- **Later:** asks again the next day.
- **I don't have it:** says who set the key and when, and how to change it. A new key does not make older backups readable: whoever has the old key should keep it until those backups are deleted.

Each admin has their own checks: everyone who may need to restore a backup proves they can. Typing the key back on the Backups page (*Check the key*) counts too. The dialog is only a reminder: it does not block anything.

### Changing the key, turning encryption off

- **Change the key:** as when turning encryption on. New backups use the new key; existing ones keep the key they were encrypted with, and the list marks those that need an earlier key. Heron cannot encrypt them again, as it cannot read them. Every other admin is asked to check the new key at once.
- **Turn encryption off:** new backups are written in clear; existing ones stay encrypted.

### Restores and the key

- An encrypted backup restores only with its key; a wrong key changes nothing. A configuration file in clear, an older backup, still restores.
- The backup taken before a restore is encrypted with the current key: undoing the restore needs that key.
- **The encryption setting and the checks are not in the configuration file.** A restore keeps the current key, keeps encryption on or off as it is, and keeps every admin's checks: restoring a backup from before a key change does not bring the old key back.

## Testing backups

A backup that was never read back is only a hope. Heron checks its backups at three levels.

### Every backup, as it is written

- **The configuration file** is read back and checked as a restore checks it (format, schema, every table) before it is encrypted. A file a restore would refuse fails the backup.
- **The database dump** must be whole: the compressed stream ends cleanly, and `mariadb-dump`'s last line, `-- Dump completed`, is there. A dump cut short fails like a dump that did not run (a `.failed` file, shown on the Backups page).
- **Checksums:** the SHA-256 of each file as written goes into `heron-checksums-<name>.sha256`, in `sha256sum`'s format: `sha256sum -c` checks a copy anywhere. Every day, before older backups are deleted, Heron checks every backup in the folder against its checksums: a file missing, cut short or changed on the share is marked **damaged** on the Backups page, and admins see a banner.

These checks need no key: Heron does them on encrypted backups too.

### With the key: a test read

Reading an encrypted backup needs the key, which Heron does not have. **Each time an admin checks the key** ([Checking the key](#checking-the-key)), Heron also reads the newest backup encrypted with it: it decrypts both files, checks the configuration file as a restore would, and decompresses the dump to its last line. Nothing is written to disk and nothing is restored: the test proves at once that the key in the admin's hands opens the backups really stored. The check answers straight away; the test runs after it, with the key kept in memory until it ends, then dropped.

*Test* on a backup of the Backups page does the same for any backup, with the key it needs (an earlier key for an older backup). Backups in clear are tested without a key: each daily backup as soon as it is taken, and the newest backup when the API starts.

The Backups page shows the last test (when, which backup, by whom, its result). Admins see a banner when the last test failed, or when no test has passed for 30 days.

### By hand: a restore drill

Only a real restore shows what no test can: a value missing from `.env`, a share that is not mounted on a new server, a step forgotten in the procedure. Restore a backup, now and then, into a **second Heron** started next to the station, from another folder, without its engine:

1. In a new folder, copy `docker-compose.yml` and the station's `.env`, then change in `.env`: `COMPOSE_PROJECT_NAME=heron-drill`, another `HERON_HTTP_PORT`, `HERON_BACKUP_FOLDER` pointing at a copy of the backups, and `HERON_STAGING_FOLDER` at an empty folder; remove every `HERON_ENGINE_URL_…`: the drill must not reach a station's engine. The library (`HERON_MUSIC_DIR`) can stay the station's: the API mounts it read-only.
2. Start the database alone, restore the dump into it ([Restoring the database dump](#restoring-the-database-dump)), then start the API and the web UI, **and nothing else**:

   ```sh
   docker compose up -d mariadb
   age -d -i heron-backup-key.txt backups/heron-database-2026-11-03T03-30-00Z-daily.sql.gz.age \
     | gunzip -c \
     | docker compose exec -T mariadb sh -c 'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" heron'
   docker compose up -d heron-api heron-web
   ```

   Do not start `heron-engine`, `heron-librarian` or `heron-analyzer`: the engine would stream to the station's own stream servers, the librarian would move files of the station's library.
3. Sign in on the drill's port and look around: stations, clocks, schedules, outputs, users. Restore the configuration file too (`heron-api backup restore … --key-file -`), to try both.
4. Remove the drill and its database: `docker compose down -v`, from the drill's folder only.

## Settings

`backups` in the `settings` table:

| Setting | Default |
|---|---|
| `enabled` | true |
| `time` | `03:30`, in the first station's time zone |
| `keep_days` | 7 |
| `database` | true: a database dump with each backup |

`backup_encryption` in the `settings` table holds the public key, when and by whom it was set, and the earlier public keys with the times they were used. It is changed only from the Backups page and its routes (`/backups/key`, see [API](api.md#routes)), never by a restore, and is not in the configuration file. `backup_tests` holds the last test and the last one that passed; neither is it in the configuration file, nor changed by a restore.
