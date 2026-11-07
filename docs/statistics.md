# Listener statistics

How many people listen, each day, each hour, to each output and to each track. Built in [M7b](roadmap/m7b-listener-statistics.md).

Listeners connect to the stream servers (Icecast, later Shoutcast), never to Heron, so Heron reads the statistics those servers keep. `heron-api` collects them; the engine is not involved and never waits for a stream server's statistics.

## Collecting

Once a minute, `heron-api` reads each [stream server](outputs.md#stream-servers) an enabled Icecast output uses, and each of its [relays](#relay-servers):

| What | Where | Needs |
|---|---|---|
| Listeners per mount | `/status-json.xsl` | nothing: the page is public |
| Connected listeners: address, player (user agent), connected for how long, the server's ID for the connection | `/admin/listclients?mount=…` | the server's **admin** credentials, an optional setting of the server |

The server is reached at its `host` and `port`, like the engine reaches it (`https` for a relay with TLS). **Each server is read once a minute**: its status page lists every mount, and each output gets its mount's count. The listener list is read once per mount.

Without admin credentials, Heron still has the **concurrent listeners** of each minute, and from them the peak, the average and the listening hours. With them, it also has each listener's **sessions**, and from them the distinct listeners of a day, the number and length of sessions, the players, and the listeners who left during each track.

Shoutcast v1 and v2 servers will be read the same way once their outputs exist ([M11](roadmap/m11-sinks-live-dj.md)). HLS has no server to ask: counting its listeners from the web server's access logs is left for later.

### Gaps

A server that does not answer within 10 seconds, or answers something unexpected, leaves that minute **without a sample**: a gap, shown as such, never a zero. Nothing about it reaches the air: the engine does not know. The statistics page and `GET /listeners/now` tell which outputs and servers could not be read and why; `heron_api_listener_polls_total{result="error"}` counts the failures.

A relay that cannot be read is a gap **for its part** only: the output's minute adds the servers that answered, and the relay's own figures (per server) have the gap.

Listeners connected through a gap keep their session: the server's ID for their connection tells it is the same one. Listeners gone when the server answers again end at the last minute they were seen.

## What is measured

| Measure | Defined as | Needs admin credentials |
|---|---|---|
| **Concurrent listeners** | Listeners of an output at a minute, as the server counts them | no |
| **Peak** | Highest concurrent listeners of the period | no |
| **Average** | Mean of the concurrent listeners over the minutes measured (gaps left out) | no |
| **Listening hours** (TLH, total listening hours) | Sum of the concurrent listeners of each minute measured, divided by 60 | no |
| **Sessions** | Listening sessions of at least the minimum length (1 min by default) that started in the period | yes |
| **Average session** | Mean length of those sessions | yes |
| **Distinct listeners** | Listeners with at least one session of the minimum length in the period | yes |
| **Departures** of a track | Sessions of the minimum length that ended while the track was on air | yes |

A **session** is one listener's connection to one output: it starts when the server says the connection started (it tells how long each listener has been connected, to the second) and ends at the last minute the listener was seen, so its end is known to the minute. A listener who reconnects within a minute (same address and player, on the same output) continues the same session. Sessions shorter than the minimum length are kept but not counted: stream directories, monitoring probes and players that check a stream before playing it make many of them, and audience measurers leave them out too. Change the minimum in the statistics settings.

A **listener** is told apart from others, within a day, by their address and player (see [Privacy](#privacy)). People sharing an address and a player (a household, an office behind one router) count as one; one person on two devices counts as two.

### Days, hours and weeks

Days and hours are in the **station's time zone** (the `station` settings, see [Rotation](rotation.md#scheduling)). When the clocks go back, the repeated hour gathers both hours' minutes; when they go forward, the skipped hour has none. A session going on at midnight is split there: it ends the day and starts the next one, so each day counts its own listeners.

Distinct listeners cannot be added up across days: a listener is not recognized from one day to the next, by design. A week or a month shows the **average distinct listeners per day** (and their sum, *listener-days*), with the peak, listening hours and sessions of the whole period.

### Whole station

The station's figures add up the outputs minute by minute: its peak is the highest sum, not the sum of the outputs' peaks. A listener on two outputs at once counts once in its distinct listeners.

## Relay servers

A station's streams are often repeated by **relay servers**: Icecast relays pulling the master's mounts, so that listeners spread over several machines. A relay is a stream server with `relay_of` set to its master ([Stream servers](outputs.md#stream-servers)): Heron sends nothing to it, no output picks it, it is only read for the statistics, with its own address, port and admin credentials. It relays all of the master's mounts, or a list of them (`relay_mounts`), with their name on the relay when it differs.

- **An output's listeners are its mount's on the master plus its mount's on each relay**, added up minute by minute like the station's figures add up the outputs. Its peak is the highest sum.
- **No double count.** A relay connects to the master as a listener of each mount it relays; that connection is left out of the master's count. It is recognized in the master's listener list by the relay's address (its `host`, resolved) and the player Icecast relays connect with (`Icecast …`). Its session is not recorded.
- **A relay behind NAT**, whose address the master does not see: set `relay_subtract_one` on it. Its connection then counts as 1 listener per relayed mount the relay has, subtracted from the master's count. The same is done when the master's listener list cannot be read (no admin credentials): the address cannot be checked.
- **Sessions and distinct listeners** count across the master and its relays as across outputs: a session is one listener's connection to one server for one output, and a listener is recognized by their address and player, so a listener who moves from the master to a relay is one distinct listener. Listeners behind a relay's own CDN keep the limitation below.
- **Per server**: the minute samples and the totals per day and hour are kept per server too, so every figure of the statistics page can be read for the master or one relay as well as for all of them. A relay's figures are its own listeners: those who connected to it (its connection to the master is left out, above).


### Per track

For each play of the music decks (not the FX player), from the [play history](api.md#play-history): the station's listeners when it started and when it ended, the change, and its departures. Over a period, tracks are compared by their **departure rate**: departures over listeners at the start, so that a track played at a busy hour does not look worse than one played at night. Tracks played fewer than 3 times in the period (`min_plays`) are left out of that ranking.

Departures are counted to the minute: a session ends when its listener was last seen, so they left within the next minute, half a minute later on average, and that is when the departure is counted. A listener who leaves in the first seconds of a track may be counted on the one before. A session split at midnight is not a departure. With many plays the error averages out; a single play says little.

## Audience figures

The figures radios report, for a period (one day or more, station time) and the filters chosen (the station or one output, all servers or one), from the sessions and the minute samples:

| Figure | Defined as |
|---|---|
| **TTSL**, total time spent listening | The sessions' lengths added up, in hours |
| **ATSL**, average time spent listening | TTSL divided by the number of sessions, with the **median** session length beside it: a few very long sessions pull the average up |
| **Cume**, unique listeners | Distinct listeners of each day (address and player, as above); for a period of several days, their average per day, with their sum (*listener-days*) |
| **AQH**, average quarter-hour audience | For each clock quarter-hour (`:00`, `:15`, `:30`, `:45`, station time) of the period measured, the listeners heard for at least 5 minutes in it, as audience measurers count; AQH is their average over those quarter-hours |
| **Peak** | The highest concurrent listeners of the period, and the minute it was reached (the first one, on a tie) |
| **Sessions** | Their number, and how many lasted under 1 minute, 1 to 5 minutes, 5 to 30 minutes, 30 minutes to 2 hours, over 2 hours |
| **Tune-in, tune-out** | For each hour of the day, the sessions that started and that ended in it, on average per day (a session split at midnight neither starts nor ends there) |

- **Sessions shorter than 5 seconds are left out** of every audience figure: players and directories probing the stream make them. This floor is fixed. It is lower than the minimum session length of the day totals (1 minute by default, [Settings](#settings)), so the short listens show in the session lengths; the cume can be higher than the day's distinct listeners for that reason.
- A session going on at midnight is split there (see [Days, hours and weeks](#days-hours-and-weeks)), so it counts as two sessions, one each day.
- The quarter-hours are fixed clock quarter-hours, not the station's dayparts. A quarter-hour counts when a listener list was read in it.
- Each figure comes with the same figure for the **previous period** of the same length (the 30 days before, for the last 30 days), to show the change.
- The audience figures need the sessions and the minute samples, kept `retention_days`: a period reaching further back counts only the days still kept. Without admin credentials, only the peak is known.

### Countries

With a **GeoIP country database**, Heron looks up each listener's address **when their session starts** and keeps only the country code (`FR`, `CA`) with the session. The address itself is never stored, as before. The lookup is local: Heron reads the database from a file, and never asks an outside service.

- **The database** is a file in the MaxMind DB format (`.mmdb`) with each network's country (`country.iso_code`). Heron is tested with the free **[IP to Country Lite](https://db-ip.com/db/download/ip-to-country-lite)** database by DB-IP: country level, updated monthly, under the [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/) licence, which asks to credit DB-IP where the results are shown (the Statistics page does). Other country databases in the same format work too, under their own licence.
- **The admin provides the file;** Heron does not bundle or download it. `HERON_GEOIP_DB` gives its path (`/var/lib/heron/geoip/country.mmdb` in the images). Without the file, countries are off: sessions get no country, and the countries card is not shown.
- **Updating it:** replace the file. `heron-api` reads it again when it changes, at the next minute; sessions keep the country they started with.
- Addresses the database does not know (private networks, a relay or CDN in front of the stream server) are **Unknown**.

With Docker Compose, the file goes into `data/geoip` next to `docker-compose.yml` (or the folder `HERON_GEOIP_FOLDER` names in `.env`), mounted read-only in `heron-api`:

```sh
mkdir -p data/geoip
# The month's IP to Country Lite database, in MMDB format, uncompressed:
gunzip -c dbip-country-lite-2026-11.mmdb.gz > data/geoip/country.mmdb
docker compose up -d heron-api
```

The API's log says `GeoIP country database loaded` with the database's type and date; *About* lists `HERON_GEOIP_DB`.

### Devices and players

The player's user agent, kept with each session, is read into:

- a **device**: mobile, desktop, smart speaker, car, TV or set-top box, other (a player that does not say);
- a **player**: the app or browser family, without its version, so that the versions of one app are one line (`VLC/3.0.20 LibVLC/3.0.20` and `VLC/3.0.21` are *VLC*; a browser is its family: Firefox, Chrome, Safari, Edge…). A user agent no rule knows is its first word (`MyApp/2.1 (Linux)` is *MyApp*).

They are read when the statistics are asked for, so better rules apply to the sessions already kept.

### Shares

Countries, devices and players are each shown with their **share of the listeners** (listener-days: each day's distinct listeners, added up) and their **share of TTSL**, with their sessions, for the period and the filters chosen. Sessions shorter than 5 seconds are left out, as for the audience figures.

**Small countries are shown as Other:** a country with fewer than **5 distinct listeners on every day of the period** is added to *Other*, so that a small station does not single out a listener ("the one listener in that country"). This floor is fixed. Devices and players are shown whatever their size: they say nothing of who listens, and with the floor a small station saw only *Other*.

## Privacy

- **IP addresses are never stored**, not even for a minute. A listener is identified by a **listener key**: a BLAKE3 hash of their address and player, keyed with a random **daily key**. The daily key is kept in the database while its day can still get sessions, then deleted, so keys of different days cannot be linked, and a key cannot be traced back to an address by hashing every possible one.
- The player (user agent) is stored with each session: it tells which apps and devices listen. **No address and no full user agent leaves the API:** the statistics routes give the device and the player family, never the user agent.
- With a GeoIP database, the session keeps its **country code only**, looked up when it starts; the address is used for the lookup and forgotten, like for the listener key.
- Countries with fewer than 5 listeners on every day of the period are shown as **Other** ([Shares](#shares)); devices and players are not grouped.
- **Sessions are kept 90 days** (the statistics settings), the minute samples too. Daily and hourly totals are kept as long as the station keeps its database; delete them with SQL if needed.
- The admin password of a stream server is stored in the database with the server, like the source password, and never sent back by the API.

## Settings

`GET`, `PUT /api/v1/settings/statistics` (admins), also on the **Settings** page:

| Setting | Default | Meaning |
|---|---|---|
| `min_session_s` | 60 | Shortest session that counts as a listener, in seconds (0 counts every one) |
| `retention_days` | 90 | How long sessions and minute samples are kept (7 to 3650) |

Changing the minimum session length recomputes the totals still covered by the sessions kept.

Per [stream server](outputs.md#stream-servers), its admin credentials (never pushed to the engine, so changing them does not reconnect its outputs):

| Setting | Default | Meaning |
|---|---|---|
| `admin_username` | `admin` | The stream server's admin user |
| `admin_password` | none | Its password: without it, Heron only counts concurrent listeners |

and, for a relay, `relay_mounts` and `relay_subtract_one` ([Relay servers](#relay-servers)).

## API

Viewers can read every statistic; the routes are under `/api/v1/listeners`. Periods are dates (`YYYY-MM-DD`, station time, both included) or Unix milliseconds for minute samples; `output_id` limits the answer to one output (the whole station otherwise), and `server_id` to one server's part (the master or a relay; every server otherwise).

| Route | Answer |
|---|---|
| `GET /listeners/now` | Each enabled output's listeners at the last minute, when that was, whether its sessions are available, the error if a server could not be read, and its part on each server (`servers`); each server's listeners (`servers`: master and relays, `null` when it could not be read); the station's total |
| `GET /listeners/samples` (`from_unix_ms`, `to_unix_ms`, at most 8 days; `output_id`; `server_id`) | Concurrent listeners minute by minute, gaps as missing minutes |
| `GET /listeners/audience` (`from`, `to`, `output_id`, `server_id`) | The [audience figures](#audience-figures) of the period and of the previous period of the same length: days measured, peak and when, sessions and their lengths, TTSL, ATSL, median session, cume, listener-days, AQH |
| `GET /listeners/days` (`from`, `to`, `output_id`, `server_id`, `group`: `hour`, `day` (by default), `week`, `month`) | Per period: days and minutes measured, peak, average, listening hours, sessions, average session, distinct listeners (for weeks and months, the average per day) and listener-days |
| `GET /listeners/hours-of-day` (`from`, `to`, `output_id`, `server_id`) | For each hour of the day (0 to 23), averaged over the days of the period: average and peak listeners, listening hours, tune-in and tune-out |
| `GET /listeners/tracks` (`from`, `to`, `output_id`, `server_id`, `min_plays` (3), `limit`) | Tracks of the period by departure rate: plays, average listeners at start and end, departures, departure rate |
| `GET /listeners/breakdown` (`from`, `to`, `output_id`, `server_id`, `by`: `country`, `device`, `player`) | The [shares](#shares) of each country, device or player: listeners (listener-days), sessions, TTSL and their shares, small groups as *Other*; whether countries are on (`geoip`: the database's type, or `null`) |

`audience`, `breakdown`, `days`, `hours-of-day` and `tracks` answer in CSV with `format=csv` (UTF-8, comma-separated, a header row, one row per figure, day, hour or track; the audience figures with a column for the period and one for the previous period), for reporting to rights societies or advertisers. Values not available (no sessions without admin credentials, a day with no minute measured) are empty in CSV and `null` in JSON.

Today's figures are computed as the minutes go by: today's row in `GET /listeners/days` changes until midnight.

## Web UI

The **Statistics** page has two filters at the top, for the whole page: the output (or the whole station) and the server (all servers, the master or a relay). Both are kept in the address (`?output=`, `?server=`), so a filtered page can be bookmarked or shared. A third filter, the **period** (the last 7, 30, 90 or 365 days, `?days=`), is the period of every card but *Today*.

The page reads like an audience report: first today live (listeners minute by minute, now, peak and listening hours so far, and each server's listeners now), then the [audience figures](#audience-figures) as tiles, each with its change against the previous period, and the session lengths; the trend over time (by day, week or month: listeners, peak, average, listening hours); the hours of the day as a **heatmap**, day of the week by hour, of the average listeners (Monday first), with the tune-in and tune-out of each hour; the countries (with a GeoIP database), the devices and the players, each as bars and a table of shares; and the tracks listeners leave. Every card follows the filters, and so does each CSV download. The charts follow the light and dark themes and fit a phone's width.

Each tile gives the figure's plain name first and its acronym after it (*Total listening time (TTSL)*, *Average listening time (ATSL)*, *Unique listeners (cume)*, *Average quarter-hour audience (AQH)*, *Peak audience*), with an info button that explains it in one sentence: on hover, on focus and on a tap, and read by screen readers as the button's description. The session lengths, tune-in/tune-out and the shares have one too. *How these figures are counted*, at the bottom of the page, sums up the rules: the 5-second floor, what a listener is (address and player, not recognized from one day to the next), the relays' own connections left out, the clock quarter-hours in the station's time zone, the comparison with the previous period, which figures need the listener sessions, and small countries shown as *Other*. **On air** shows each output's current listeners, and the station's total.

## Metrics

`heron-api` exports `heron_listeners{output}`, each enabled output's listeners at the last minute (`NaN`, a gap in graphs, while its server cannot be read), and `heron_api_listener_polls_total{result}` (`ok`, `error`). The Grafana dashboard shows them in its **On air** row. See [Monitoring](monitoring.md#heron-api).

## Storage

See [Data model – Listener statistics](data-model.md#listener-statistics). Every minute, the collector writes the minute's samples, opens, continues and closes sessions, then recomputes the totals of the current hour and day (and of the previous ones, whose sessions may just have ended). The totals tables are small: one row per output, server and hour, and for all of them. When the API first starts with per-server totals, it recomputes the totals of the days still covered by the sessions and samples kept, once.

## Limitations

- The totals per server start with this feature: days whose samples were already purged have the totals of all servers only.
- A relay is recognized in the master's list by the address its `host` resolves to: give its address as the master sees it, or set `relay_subtract_one`.
- A country is only as right as the GeoIP database: country-level free databases are right for most addresses, less for mobile networks and VPNs.
- The device and player come from what the player says it is; some say nothing useful (*Other*).
- Only Icecast for now: Shoutcast with its outputs ([M11](roadmap/m11-sinks-live-dj.md)), HLS later.
- A minute's sample is the count at the moment of the request: a listener who stays less than a minute may be missed by the concurrent counts (sessions still see them, with admin credentials, when their connection lasts across a request).
- Statistics start when the API first reads the server; nothing is recovered for the time the API was down.
- Listeners going through a relay or a CDN in front of Icecast all appear with the relay's address: they are counted, but a reconnecting listener behind it cannot be told apart from another one with the same player.
