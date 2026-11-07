# Audience certification

A station can have its online audience **certified** by a recognized body. These bodies measure streaming audience from the **stream servers' access logs**: one line per listener session, with the address, the player, the mount, when it ended and how long it lasted. Heron reads those logs, keeps the sessions for the retention period, and exports them as the bodies ask. Built in [M19](roadmap/m19-audience-certification.md).

This is not the [listener statistics](statistics.md): those are rebuilt by Heron from the servers' listener lists, read once a minute. A certifier wants the server's own log, unchanged, and applies its own rules.

## The access logs

### What is read

Each [stream server](outputs.md#stream-servers), relays included, can have an **access log** (*Outputs › Servers*, the server's *Access log*). Icecast writes one line per session when the listener leaves, in the *combined* format, with the session's length in seconds at the end:

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

**Rotation:** when the file is shorter than the position, it was rotated. Heron first reads the end of the previous file, `<path>.1` or `<url>.1` when it exists, then starts the new file from the beginning. Without a previous file, the lines written between the last reading and the rotation are lost; the server's page says so, with when.

**A server Heron cannot read** shows the error and since when on its page and in the [status](monitoring.md), and nothing is guessed: the gap stays, and the export says which hours it misses.

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

Each line is kept with its server, its station and what was parsed from it, and the line itself, unchanged. Sessions are kept **13 months** by default (*Settings › Certification*, 1 to 25 months), then deleted with their lines, every night. The positions in the logs are kept with the servers.

## Export

For a **day** (in the station's time zone), a **server** and optionally a **mount**, from *Statistics › Certification* and from the API:

| Format | What it is |
|---|---|
| `raw` | The server's lines, unchanged and in their order: what the bodies take |
| `combined` | The sessions as Icecast combined log lines, the same as `raw` without the lines that are not sessions |
| `csv` | One row per session: `server`, `mount`, `address`, `user_agent`, `referer`, `status`, `bytes`, `started_at`, `ended_at`, `duration_s` (times in UTC, ISO 8601) |
| `json` | The same, as an array |

A day takes the sessions that **ended** that day, as the server logged them. Nothing is filtered or merged: short sessions, robots and repeated addresses are the certifier's to judge. A day with a gap in the reading says so: the export has its hours in a header (`X-Heron-Gaps`), and the page warns before downloading.

## Automatic delivery

*To come in the second part of M19:* per certification body and mount, a delivery by SFTP or FTPS at a set time, retried, with its failures alerted and the last 30 listed. Presets per body come once each body's specification is obtained.

## Privacy

The access logs hold the listeners' **addresses** and players: personal data.

- Kept for the retention period only, then deleted; turning the module off does not keep them longer.
- **Never shown** in the web interface: the page gives counts, gaps and the downloads, not the lines.
- **Admins only** download exports. Each download is logged with who, which day and which server.
- The access log's password is never sent back by the API.
- What is sent to whom is the station's to say in its privacy notice: the certifier receives the exports, and nothing else leaves Heron.

## Module

*Certification* (`certification`) is a [module](modules.md), off until an admin turns it on. Off, no log is read and its routes answer `404`; the sessions kept are still deleted after their retention.

## API

- `GET`, `PUT /stream-servers/{id}` gain `access_log`: `null`, or `{"source": "file", "path"}`, or `{"source": "url", "url", "username", "password"}` (the password is write-only). `GET /stream-servers/{id}/access-log` gives its state: `position`, `read_at_unix_ms`, `error` and since when, the lines and sessions read today.
- `GET`, `PUT /settings/certification` (admins): `retention_months` (1 to 25, 13).
- `GET /certification/days?from&to` (admins): per day and server, the sessions, the lines and the gaps.
- `GET /certification/export?day=2026-11-07&server_id=3&mount=/live&format=raw|combined|csv|json` (admins): the file, named `<station>-<server>-<day>.<ext>`; `X-Heron-Gaps` lists the hours missing.

## Limitations

- Icecast only, like the stream servers. Shoutcast comes with its outputs ([M11](roadmap/m11-sinks-live-dj.md)).
- HLS has no server session to log.
- No log shipper is built in: a remote server needs a web server to serve its log.
