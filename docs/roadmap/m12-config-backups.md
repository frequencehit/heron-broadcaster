# M12 – Backups

**Depends on:** M4, M13 (stations), M14 (schedules).

## Why

A station is set by hand, over months: stations, clocks, schedules, categories, restrictions, outputs and their passwords, settings, webhooks, users, and on each track its categories, cue points and markers. Losing that to a bad change, a broken disk or a database restored from the wrong day costs far more than the music files, which rebuild the library. Design: [Backups](../backups.md).

## Scope

- **Daily backup** by `heron-api` at a set time (03:30 by default) of two files: the **configuration file** (JSON: everything set by hand, what is set on each track included) and a **database dump** (`mariadb-dump`, compressed: history and statistics included). The last 7 days kept (a setting, 1 to 90). Secrets included, the files readable by the API's user only.
- **The backup folder** is a folder of the host (`HERON_BACKUP_FOLDER`, `./data/backups` by default): pointed at a network share or a synchronized folder, it keeps copies off the machine.
- **Manual backup** at any time, kept until deleted.
- **Restore** of a whole configuration file, by admins, from the web UI, the API or `heron-api backup restore`: checked first, the current configuration backed up first, applied in one transaction, sessions closed, each engine updated at once. Tracks, artists, categories, stations… referred to by path, name or code, so a backup restores on top of a rebuilt library.
- **The database dump** is restored with MariaDB's tools, as documented.
- **List, download, upload, delete** backups.
- **Backups page** in the web UI (admins).

## Out of scope

- Artist names moved or merged by hand: they would need the tracks linked again. Artists marked similar and suggestions decided on are restored.

- Restoring part of a backup (only the clocks, only the outputs, one station).
- Restoring the database dump from the web UI.
- Sending backups to object storage (S3…): the backup folder can be a mounted share.
- Encrypting the files.

## Deliverables

- `heron-api`: the configuration file (format 1), the database dump, the daily schedule and retention, `backup create|list|restore` subcommands, routes under `/api/v1/backups`, the `backups` settings group. `mariadb-dump` in its image.
- `docker-compose.yml`: the backup folder on `heron-api`, `HERON_BACKUP_DIR`, `HERON_BACKUP_FOLDER`.
- `heron-web`: the Backups page.
- Docs: [Backups](../backups.md), [API](../api.md), [Data model](../data-model.md#settings), [Web UI](../web.md#pages), the README.

## Acceptance criteria

- [ ] A Heron with two stations, configured with every kind of setting the file holds and settings on tracks, is backed up, its database dropped, Heron started on an empty database and the library scanned; restoring the configuration file gives back every station, setting, output (streaming without retyping a password), clock, schedule and its periods, category, restriction, label, medley, webhook, user and API token, and every track's categories, weights, labels, overrides, markers and programming, identical through the API.
- [x] What names a track or an artist is restored by path and name onto the rebuilt library; what names one that is gone is reported, not restored.
- [x] Restoring the database dump with the documented commands gives back the history and listener statistics.
- [x] The daily backup runs at its time, catches up when the API was down at that time, and keeps exactly the last 7 daily backups; manual and pre-restore backups are kept as described; a backup's two files go together.
- [x] A restore is undone by restoring the backup it took first.
- [ ] A corrupt, truncated or newer-format file is refused with a clear message and changes nothing; a crash during a restore leaves the configuration as it was before it.
- [x] Backup files are written atomically and readable by the API's user only; only admins can list, download, upload, restore or delete them.
- [ ] The engines play on through a backup and a restore; outputs whose settings did not change do not reconnect.

## References

- [Backups](../backups.md)
- [API – Engine configuration](../api.md#engine-configuration)
- [Data model](../data-model.md)
