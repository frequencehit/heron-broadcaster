# M19 – Audience certification

**Depends on:** M7b (stream servers and listener statistics), M13 (stations), modules

## Why

A station whose online audience is certified by a recognized body can show audited figures to advertisers. These bodies measure from the stream servers' access logs, which the station's streaming provider sends them today. Heron runs or knows those servers: it can read their logs, keep them for the time asked, and export them, then send them by itself ([#189](https://github.com/frequencehit/heron-broadcaster/issues/189)).

The bodies' exact formats and delivery channels come with each agreement and are not public: this milestone builds the generic part, and leaves a preset per body for when its specification is obtained.

## Scope

Part 1, collecting and exporting:

- **An access log per stream server**, relays included: a file under the access logs folder mounted read-only in `heron-api`, or an HTTPS address read with range requests, with an optional login.
- **Read once a minute from the position kept**, so that no line is read twice, across restarts too; log rotation followed through the previous file (`.1`); a server that cannot be read shows its error and since when, and the hours missed are kept as gaps.
- **Lines kept unchanged with what is parsed from them:** address, mount, status, bytes, referer, User-Agent, end and length of the session. Lines that are not a listener's are kept but are not sessions.
- **Retention:** 13 months by default, 1 to 25, then deleted every night.
- **Export** for a day, a server and optionally a mount: `raw`, `combined`, `csv` and `json`, from the *Certification* page and the API, admins only, each download logged; the gaps of the day said.
- **A `certification` module**, off by default.

Part 2, delivery (a second pull request):

- Per certification body and mount: SFTP or FTPS (HTTPS upload if a body needs it), host, credentials (write-only), path, file name pattern, time of day, compression.
- Retried, logged, alerted on failure; the last 30 deliveries listed with their state.

## Out of scope

- Presets per body (format, fields, file name, schedule): each comes once the body's specification is obtained.
- A built-in log shipper: a remote server serves its log over HTTPS.
- Shoutcast servers (with their outputs, [M11](m11-sinks-live-dj.md)) and HLS, which has no server session.
- Showing the lines or the addresses in the web interface.

## Deliverables

- `heron-api`: the access log settings of the stream servers, the reader (file and address, positions, rotation, gaps), the storage and its retention, the export routes, the module.
- `heron-web`: a *Certification* page after Statistics (admins): each server's access log, its state, the days and the downloads, the retention.
- Migrations: the access log settings and positions, the sessions and their lines, the gaps, the download log.
- Docker Compose: the access logs folder, read-only.
- Docs: [Audience certification](../certification.md), [Outputs – Stream servers](../outputs.md#stream-servers), [Modules](../modules.md), [API](../api.md), [Web UI](../web.md), CHANGELOG.

## Acceptance criteria

Part 1:

- [ ] With an access log in a file and another at an address, a day's lines are read once each, across a restart of `heron-api` and a rotation of the log, and kept unchanged.
- [ ] A day's sessions export in `raw`, `combined`, `csv` and `json`, per server and mount; `raw` is byte for byte the server's lines of that day.
- [ ] A server that cannot be read shows its error; the hours missed show as gaps on the page and in the export.
- [ ] Sessions older than the retention are deleted; nothing in the web interface shows an address; only admins download, and each download is logged.
- [ ] Off, the module reads nothing and its routes answer `404`.
- [ ] Unit tests for the parser (Icecast combined lines, odd lines, time zones), the positions and the rotation; an end-to-end test from a file to each export.
- [ ] Docs, translations (all catalogs) and the CHANGELOG are up to date.

Part 2:

- [ ] A delivery sends the day's file by SFTP at the set time, retries, and its failures are visible and alerted.

## References

- [Audience certification](../certification.md)
- [Listener statistics](../statistics.md)
- [Outputs – Stream servers](../outputs.md#stream-servers)
