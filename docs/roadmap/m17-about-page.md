# M17 – About page

**Depends on:** M5 (the web UI), M13 (roles), M16 (the status bar)

## Why

To report a problem, or to check an upgrade went through, one needs to know which build runs, on what machine, with which features on. Today the pieces are scattered: the version in a banner shown only when the web UI and the API differ, the services' versions behind `/healthz`, the configuration in environment variables only the person who deployed can read. One page gathers them, and a button copies them for a bug report.

## Scope

- **An About page**, linked at the bottom of the navigation next to Account, at `/about`.
- **Build** (every role): the Heron version, the commit it was built from, whether the web UI and the API come from the same build, the version of each service (API, engine, analyzer, librarian) with whether it answers, the build date.
- **Runtime** (admins): how long each service has been running, the operating system, kernel and architecture of the machine, whether Heron runs in a container, the time zone and the clock's synchronization, the CPU count and memory.
- **Dependencies** (admins): the database server and its version, the applied migrations, the FFmpeg version when the external encoder is set (`built-in` otherwise).
- **Active configuration** (admins, read-only): which features are on (library and incoming folder watching, error reporting, metrics remote write, external FFmpeg, the monitor output, the number of stations) and the values that are not secret (folders, intervals, worker counts, listen addresses). A secret shows only as *set* or *not set*.
- **Links:** documentation, changelog, repository, license.
- **Copy diagnostics:** one button puts everything the user may see on the clipboard as plain text, to paste into a bug report.
- **One route**, `GET /api/v1/about`, answers with the build part to every role and the other parts to admins only.
- **How the services report:** the engine answers a `GetAbout` call over gRPC; the analyzer and the librarian, which have no address the API knows, write a heartbeat to the database every 30 seconds (`service_heartbeats`: version, start time, machine, configuration). A heartbeat older than 90 seconds means the worker is not running. The table is not in the configuration file of a backup (the database dump holds it like any table, and a restored one is overwritten by the next heartbeat).

## Out of scope

- Changing any value: the page only reads. Settings stay where they are.
- The version of the stream servers (Icecast): they do not tell it in a form every server shares.
- History of versions or of upgrades.
- Checking for a newer release.
- Disk space, temperature and per-process figures: see [Monitoring](../monitoring.md#what-else-could-be-monitored).

## Deliverables

- `heron-api`: `GET /api/v1/about`, aggregating the services' own reports; a field-by-field list of the configuration shown (never a list of what is hidden).
- `heron-engine`: the `GetAbout` call. `heron-analyzer`, `heron-librarian`: the heartbeat and a migration for its table.
- Images: `HERON_REVISION` and `HERON_BUILD_DATE` built into all four Rust images.
- `heron-web`: the page, the navigation link, translations (all catalogs), the copy button.
- Docs: [Web UI](../web.md#pages), [API](../api.md#routes).

## Acceptance criteria

- [x] The page is linked at the bottom of the navigation and works from a phone up, in light and dark themes.
- [x] A viewer or operator receives and sees the build part and the links only; an admin sees every part.
- [x] A secret (database URL, error-reporting DSN, metrics credentials, and any added later) never appears in the answer, whatever the configuration: a test sets them and searches the answer.
- [x] With a service stopped, the page says so for that service and still shows the others.
- [x] The web UI and the API from different builds show as different, as the banner does.
- [x] *Copy diagnostics* copies what the user sees, and nothing more.
- [x] Unit tests for the route and its roles, the configuration list and the page; docs, translations (all catalogs) and screenshots are up to date.

## References

- [Web UI – Status bar](../web.md#status-bar)
- [API – Authentication](../api.md#authentication)
- [Monitoring](../monitoring.md)
