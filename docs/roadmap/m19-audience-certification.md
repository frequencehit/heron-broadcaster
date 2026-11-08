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

- **Per certification body, server and mount:** SFTP or FTPS (explicit TLS), host and port, credentials (a password, or an SSH private key for SFTP; write-only), folder, file name pattern (station, server, mount and date tokens), format, compression (none or gzip), time of day in the station's time zone, the day sent (the day before by default), enabled.
- **SFTP host keys trusted on first use:** the first connection that logs in keeps the key's fingerprint, shown; a changed key is refused until it is forgotten. A fingerprint can be typed first. FTPS checks the certificate with the system's authorities.
- **Sent every day at its time**, a day with gaps too (the gaps in its log); **retried** 5 times over about 2 hours; **logged** (each attempt: when, result, error, file and size); a failure **alerted** through a `certification.failed` webhook event and a metric with its alert rule; the last 30 days sent listed with their state; *Send now* for a chosen day; *Test connection*.
- No HTTPS upload: no body is known to ask for one yet.

## Out of scope

- Presets per body (format, fields, file name, schedule): each comes once the body's specification is obtained.
- A built-in log shipper: a remote server serves its log over HTTPS.
- Shoutcast servers (with their outputs, [M11](m11-sinks-live-dj.md)) and HLS, which has no server session.
- Showing the lines or the addresses in the web interface.

## Deliverables

- `heron-api`: the access log settings of the stream servers, the reader (file and address, positions, rotation, gaps), the storage and its retention, the export routes, the module; the deliveries, their schedule, the SFTP and FTPS senders, retries, their log and the alerts.
- `heron-web`: a *Certification* page after Statistics (admins): each server's access log, its state, the days and the downloads, the retention; the deliveries, their form, their log, *Send now* and *Test connection*.
- Migrations: the access log settings and positions, the sessions and their lines, the gaps, the download log; the deliveries, the days sent and their attempts.
- Docker Compose: the access logs folder, read-only.
- Monitoring: the deliveries' metric and alert rule.
- Docs: [Audience certification](../certification.md), [Outputs – Stream servers](../outputs.md#stream-servers), [Modules](../modules.md), [API](../api.md), [Web UI](../web.md), [Monitoring](../monitoring.md), CHANGELOG.

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
- [ ] The file sent is the day's export, named from its pattern, compressed when asked; a day with gaps is sent with its gaps in the log.
- [ ] The SFTP host key is kept on the first connection and a changed one is refused; passwords and keys are never sent back.
- [ ] The last 30 days sent are listed with their attempts; *Send now* and *Test connection* work; only admins, and off, the module sends nothing.
- [ ] An end-to-end test delivers by SFTP to a server in the test (the bytes received, a refused login retried, the alert, the 30 days kept); FTPS is tested the same way against a server in the test.
- [ ] Docs, translations (all catalogs) and the CHANGELOG are up to date.

## References

- [Audience certification](../certification.md)
- [Listener statistics](../statistics.md)
- [Outputs – Stream servers](../outputs.md#stream-servers)
