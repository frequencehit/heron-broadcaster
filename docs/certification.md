# Audience certification

A station can have its online audience **certified** by a recognized body. These bodies measure streaming audience from the **stream servers' access logs**: one line per listener session, with the address, the player, the mount, when it ended and how long it lasted. Heron reads those logs, keeps the sessions for the retention period, exports them as the bodies ask, and sends them to the bodies by itself. Built in [M19](roadmap/m19-audience-certification.md).

This is not the [listener statistics](statistics.md): those are rebuilt by Heron from the servers' listener lists, read once a minute. A certifier wants the server's own log, unchanged, and applies its own rules.

## The access logs

### What is read

Each [stream server](outputs.md#stream-servers), relays included, can have an **access log** (the *Certification* page, admins). Icecast writes one line per session when the listener leaves, in the *combined* format, with the session's length in seconds at the end:

```
203.0.113.7 - - [07/Nov/2026:20:41:05 +0100] "GET /live HTTP/1.1" 200 5242880 "https://radio.example.org/" "VLC/3.0.20 LibVLC/3.0.20" 1342
```

A line is a session: the address, the mount (`/live`), the status, the bytes sent, the referer, the User-Agent, its end (the date) and its length (`1342` seconds; its start is the end minus the length). Lines that are not a listener's (the source's `SOURCE` or `PUT`, the admin's `/admin/…`, status pages) are kept for the raw export but are not sessions.

### Where it is read from

| Source | Setting | For |
|---|---|---|
| **A file** | `path`, under the access logs folder mounted in `heron-api` | A server on the same host: its log folder is mounted read-only |
| **An address** | `url`, with an optional `username` and `password` (never sent back) | A server on another host: its log file served over HTTPS by the web server next to it, behind a login |

Heron reads each log **once a minute**, from where it stopped: the position is kept per server, so no line is read twice, even across restarts. An address is read with HTTP range requests (`Range: bytes=<position>-`): the web server must allow them, which serving a static file does.

**Rotation:** when the file is shorter than the position, it was rotated. Heron first reads the end of the previous file, `<path>.1` or `<url>.1` when it exists, then starts the new file from the beginning. Without a previous file, the lines written between the last reading and the rotation are lost: a gap is kept, with when.

**A server Heron cannot read** shows the error and since when on the *Certification* page, and nothing is guessed: once it is read again, the time it could not be is kept as a gap, which the days and the export show.

### Deployment

A local server's log folder is mounted in `heron-api`, read-only:

```yaml
heron-api:
  volumes:
    - ${HERON_ACCESS_LOGS_FOLDER:-./data/access-logs}:/var/lib/heron/access-logs:ro
```

Icecast writes its access log into a subfolder of it (`<accesslog>access.log</accesslog>` with `<logdir>` pointing there), and the server's path is relative to the folder (`main/access.log`).

A remote server's web server serves the file, behind a login, for example `https://stream.example.org/logs/access.log`. The log should be rotated **daily, a little after midnight**, keeping the previous file next to it.

## Storage and retention

Each line is kept with its server, its station and what was parsed from it, and the line itself, unchanged. Lines are kept **13 months** by default (the *Certification* page, 1 to 25 months, for the installation), then deleted every hour with the gaps and the download log. The positions in the logs are kept with the servers. The access log settings and the deliveries are not in the [configuration backups](backups.md) yet; the lines and the deliveries are in the database dump.

## Export

For a **day** (in the station's time zone), a **server** and optionally a **mount**, from the *Certification* page and from the API:

| Format | What it is |
|---|---|
| `raw` | The server's lines, unchanged and in their order: what the bodies take |
| `combined` | The sessions as Icecast combined log lines, the same as `raw` without the lines that are not sessions |
| `csv` | One row per session: `server`, `mount`, `address`, `user_agent`, `referer`, `status`, `bytes`, `started_at`, `ended_at`, `duration_s` (times in UTC, ISO 8601) |
| `json` | The same, as an array |

A day takes the sessions that **ended** that day, as the server logged them. Nothing is filtered or merged: short sessions, robots and repeated addresses are the certifier's to judge. A day with a gap in the reading says so: the export lists them in a header (`X-Heron-Gaps`, ISO 8601 intervals in UTC, comma-separated), and the page shows them next to the day.

## Automatic delivery

A **delivery** sends a day's export to a certification body by itself, every day at a set time, by FTPS: per body, server and optionally mount (the *Certification* page, admins). SFTP comes next (see [Limitations](#limitations)). Presets per body come once each body's specification is obtained: until then, set the format, the file name and the schedule the body asks for.

### What a delivery sets

| Setting | What it is |
|---|---|
| `name` | The body it sends to, as the page shows it |
| `server_id`, `mount` | The server whose log is sent, and only this mount (`/live`), or every mount |
| `protocol` | `ftps`: FTP with explicit TLS (`AUTH TLS`), passive mode |
| `host`, `port` | The body's server; port 21 by default |
| `username`, `password` | The login. The password is **write-only**, like the stream servers' passwords |
| `remote_dir` | The folder to write into, on the body's server (`/incoming`); empty, the login's folder |
| `file_name` | The file's name, from a pattern, see [File names](#file-names) |
| `format` | `raw`, `combined`, `csv` or `json`, as the [export](#export) gives them; `raw` by default |
| `compression` | `none`, or `gzip` |
| `send_at` | The time of day, `HH:MM` in the station's time zone; `04:00` by default |
| `days_back` | Which day it sends: `1` the day before (by default), up to `7` days before |
| `enabled` | Off, it is not sent at its time; *Send now* still sends it |

### File names

The pattern is made of text and tokens: `{station}-{server}-{date}.{ext}` by default, which gives `main-Main-2026-11-07.log`.

| Token | Is |
|---|---|
| `{station}` | The station's code |
| `{server}` | The server's name |
| `{mount}` | The mount without its `/` (`live`), `all` for every mount |
| `{date}` | The day sent, `2026-11-07` |
| `{yyyy}`, `{mm}`, `{dd}` | Its year, month and day, for other orders (`{yyyy}{mm}{dd}` is `20261107`) |
| `{ext}` | `log`, `csv` or `json` from the format, with `.gz` when compressed (`log.gz`) |

Values are written with letters, digits, `-` and `_` only. An unknown token, a `/` or an empty name is refused (`422`). The file is written as it is named, replacing one of the same name.

### The server's certificate

FTPS checks the server's certificate against the system's certificate authorities, with the host name (or the address), like the access logs' addresses; the files go over the encrypted connection only (`PROT P`). A server whose certificate is not signed by a known authority is refused.

### When it is sent

Every day at its time, Heron sends the day chosen: the lines that **ended** that day, in the station's time zone, as the export gives them. A delivery created after its time of the day sends the next day first. If `heron-api` was stopped at the time, the day is sent when it starts again, that same day; a day missed altogether is sent with *Send now*.

A day with **gaps** is still sent: the delivery lists them in its log, as the export does, and the page shows them. A log that cannot be read at the time adds a gap from when it could not.

**Retries:** an attempt that fails (no connection, a certificate refused, a login refused, a folder that does not exist) is tried again 5 minutes later, then 15 and 30 minutes and 1 hour later: **5 attempts over about 2 hours**. Then the delivery has **failed**:

- it shows on the *Certification* page, with its error;
- a [webhook](api.md#webhooks) with the `certification.failed` event gets it, with its `message` (the delivery's name, the day and the error);
- `heron_api_certification_deliveries_failing` counts the deliveries whose last day failed, for the `HeronCertificationDeliveryFailed` [alert](monitoring.md#alerts).

A day sent again later, by *Send now* or at the next time, clears the failure when it goes through.

### The log

Each delivery keeps its **last 30 days sent**, by its time or *Send now*: the day, its state (`pending`, `sending`, `delivered`, `failed`), the file's name and size, the gaps, and each attempt with when, its result and its error. *Send now* sends a chosen day at once, with the same retries; *Test connection* connects and logs in, opens the folder and writes nothing.

## Privacy## Privacy

The access logs hold the listeners' **addresses** and players: personal data.

- Kept for the retention period only, then deleted; turning the module off does not keep them longer.
- **Never shown** in the web interface: the page gives counts, gaps and the downloads, not the lines.
- **The download log** (`certification_downloads`: who, when, which day, server, mount and format) is kept as long as the lines.
- **Admins only** download exports. Each download is logged with who, which day and which server.
- The access log's password is never sent back by the API.
- What is sent to whom is the station's to say in its privacy notice: the certifier receives the exports, and nothing else leaves Heron. The deliveries name each body, and their log says which days went where.
- The deliveries' passwords are never sent back by the API. Like the stream servers' passwords, they are kept in the database, and in its dump.

## Module

*Certification* (`certification`) is a [module](modules.md), off until an admin turns it on. Off, no log is read, nothing is delivered and its routes answer `404`; the sessions kept are still deleted after their retention.

## API

Admins only, but `GET /settings/certification`:

- `GET`, `PUT`, `DELETE /stream-servers/{id}/access-log`: where the server's log is read, `{"source": "file", "path"}` or `{"source": "url", "url", "username", "password"}` (the password is write-only: left out, it is kept; `null` removes it), and how its reading goes: `position`, `read_at_unix_ms`, `read_ok_at_unix_ms`, `error` and `error_since_unix_ms`. A new file or address starts from its beginning.
- `GET /certification/sources`: the station's servers with an access log, the same way.
- `GET`, `PUT /settings/certification`: `retention_months` (1 to 25, 13).
- `GET /certification/days?from&to` (62 days at most): per day and server, the lines, the sessions among them and the gaps.
- `GET /certification/export?day=2026-11-07&server_id=3&mount=/live&format=raw|combined|csv|json`: the file, named `<station>-<server>-<day>.<ext>`; `X-Heron-Gaps` lists the gaps.
- `GET`, `POST /certification/deliveries`; `GET`, `PUT`, `DELETE /certification/deliveries/{id}`: the [deliveries](#what-a-delivery-sets). `password` is write-only (`password_set` says whether one is kept; left out, it is kept). Each delivery comes with its `last` day sent.
- `POST /certification/deliveries/{id}/test`: connects, logs in and opens the folder, answers `ok` and `error`.
- `POST /certification/deliveries/{id}/send` with `{"day": "2026-11-07"}`: sends that day now (`202`, the day sent as `GET …/runs` gives it).
- `GET /certification/deliveries/{id}/runs`: its last 30 days sent, newest first, each with its `attempts`.

## Limitations

- Icecast only, like the stream servers. Shoutcast comes with its outputs ([M11](roadmap/m11-sinks-live-dj.md)).
- HLS has no server session to log.
- No log shipper is built in: a remote server needs a web server to serve its log.
- Deliveries go by FTPS only for now: no plain FTP (the logs hold personal data), no HTTPS upload until a body asks for one. **SFTP comes next**: the pure Rust SSH library Heron would use needs a newer version of a cryptography crate than the one the backups' encryption library (age) is built with, and the two cannot be in the same build. It comes once age is released with it.
