# Several stations

One Heron can run **several stations**: one library, one buffer zone, one analysis, and several stations on air, each with its own programming, playout and outputs. A group of stations (a main station and themed sister stations, say an 80s and a 90s one) shares its music, its artists and its tags, programs each station on its own, and keeps them apart with rules across stations. Built in [M13](roadmap/m13-several-stations.md).

A Heron with one station works as before: the station is there, and nothing asks to choose one.

## Stations

A **station** has:

| Field | |
|---|---|
| **Name** | Shown in the web UI, e.g. `Hits`, `Hits 80` |
| **Code** | Short and unique, lowercase letters and digits (e.g. `hits`, `hits80`): shown in badges next to categories and medleys, used in the API's public routes, and names the variable of its engine's address. Set once for the stations after the first; the first station's can be changed |
| **Time zone** | Its clocks, schedule, restrictions and statistics follow it: its `station` settings |
| **Engine** | The engine that puts it on air, one per station: its address is in `heron-api`'s environment, not in the database ([Engines](#engines)) |
| **Enabled** | A disabled station is not fed: its engine plays on with what it has queued, then its fallback directory |

Admins create, rename and disable stations. A station is deleted only once nothing refers to it but its history and statistics, which go with it.

An existing Heron becomes **one station**, named after the station name setting (`HERON_STATION_NAME`), with code `main` until renamed: all its categories, clocks, schedule, history, outputs and settings become that station's.

## What is shared, what is per station

| | Shared by all stations | Per station |
|---|---|---|
| **Library** | Tracks, files, analysis, cue points and overrides, intro and hook, tags, labels, programming details (era, mood, BPM…) | — |
| **Artists** | Artists, their names, similar links | Plays and last play, separation windows |
| **Categories** | Categories and category types, a track's categories and weights | Which categories a station can use: those tagged with it ([Categories](#categories)) |
| **Programming** | — | Clocks, schedule, rotation settings (separation windows, fallback category, weights), selection log, simulations |
| **Restrictions** | — | Each restriction applies to the stations it names, all by default ([Restrictions](#restrictions)) |
| **Queue and playout** | — | Queue, autofill, playout (crossfade, loudness), deck EQ; EQ presets are shared |
| **History** | Plays in total | Each play is a station's ([Plays](#plays)) |
| **Medleys** | Medleys, their recipes and parts, rendered files | Which stations a medley plays on ([Medleys](#medleys)) |
| **Outputs** | Encoder profiles | Outputs, their listener statistics |
| **Webhooks, public endpoints** | — | Per station ([API](#api)) |
| **Users** | Users, sign-in, API tokens | Roles ([Roles](#roles)) |
| **Configuration backups** | One backup holds every station | — |

### Settings

| Settings group | Shared | Per station |
|---|---|---|
| `analysis` (Gap Killer, crossfade points) | ✓ | |
| `library` (AcoustID key, layout, covers) | ✓ | |
| `programming` (what counts as a play, eras, mood and sound code scales) | ✓ | |
| `statistics` (minimum session, retention) | ✓ | |
| `backups` | ✓ | |
| `medleys`: `name_from`, `defaults` | ✓ | |
| `medleys`: rendered medleys' category, liner categories | | ✓ |
| `station` (time zone) | | ✓ |
| `playout` (crossfade, loudness) | | ✓ |
| `eq` (the music and FX EQs) | | ✓ |
| `queue` (autofill, tracks waiting) | | ✓ |
| `engine` (whether the API manages the engine's configuration) | | ✓ |
| `rotation` (separation windows, rules across stations, fallback category, weights) | | ✓ |

The web UI's **Settings** keeps the shared groups; each station's own are under that station.

## Categories

A category is **tagged with the stations it is used on**: one, several or all. With stations `hits`, `hits80` and `hits90`:

| Category | Stations | |
|---|---|---|
| `80s` | `hits`, `hits80`, `hits90` | Used on the three |
| `Hits gold` | `hits` | Only on `hits` |
| `80s gold` | `hits80` | Only on `hits80`, until it is tagged `hits90` too, or renamed |

- The stations' codes show as badges next to the category's name wherever it appears: the track page, the library's filters, clocks, restrictions, medleys.
- A station's clocks, restrictions, fallback category and medley settings only offer the categories tagged with it.
- **Untagging is refused while the station uses the category**, listing what uses it: its clocks' slots, its restrictions, its fallback category, its medley settings, the recipes of the medleys tagged with it. The station stops using it first.
- A category is tagged with at least one station. A new category is tagged with the station being worked on; categories brought by an import from another software are tagged with the stations the import chose.
- A track's categories and weights are the same on every station: a track in `80s` plays on the three stations above, with its one weight.

Category types (Song, Jingle, Ad…) are shared.

## Restrictions

A restriction (daypart, date range, category sequence) **applies to the stations it names**, all by default: a track can be kept out of the mornings on one station only. Its stations show next to it on the Restrictions page and on the track, artist or category it targets.

## Plays

- Each play in the history is a station's (`play_history.station_id`), recorded from that station's engine.
- A track and an artist show their **plays and last play per station** and **in total**: the track and artist pages (`plays`), the library list and its sort and filters (`station_id`: a station's plays; without it, every station's).
- Separation, weights and the selection log of a station only look at that station's plays, except for the [rules across stations](#rules-across-stations).
- Listener statistics are per output, so per station, and for a whole station (an output's totals with no output: the station's).

## Rules across stations

A station can keep its music apart from what other stations play: in its `rotation` settings, **windows across stations**, each off by default:

| Rule | Do not play here what another station played or will play within |
|---|---|
| Artist | N minutes (e.g. 20 or 30) |
| Title | N minutes |
| Track | N minutes |

- **Which stations** a station looks at: all others, or some.
- The window looks **both ways**: what the other stations played, and what their queue will play at its planned times, so two stations planning at the same moment do not pick the same artist for the same minutes. A pick that another station then takes too is caught by whichever station plans later.
- Artists are compared as the station's own rule does: the same artist or a similar one ([Artists with several names](rotation.md#artists-with-several-names)).
- **Relaxation:** each rule across stations is halved then dropped with the station's own rule of the same kind (artist at steps 3 and 4, title at 5 and 6, track at 7 and 8, [Rule relaxation](rotation.md#rule-relaxation)), so it holds as long as the station's own does.
- The **selection log** says how many tracks the rules across stations held back, and which station's play or planned track held back the one a slot wanted; a simulation of a station takes the other stations' history and queues as they are, and does not simulate them.

## Medleys

A medley is **tagged with the stations it plays on**, like a category: one, several or all.

- Its rendered track goes into the **rendered medleys' category of each of its stations** (a `medleys` setting of each station), so each station's clocks and FX player find it there. A station without that category set cannot be tagged.
- Its songs come from **every category its stations can use** when its recipe chooses none; its recipe can narrow them to some categories (only `Hits gold`, or only `80s gold`), as today. The recipe only offers categories tagged with at least one of its stations.
- Its liners come from the liner categories of **its first station** (the one it was made on, shown first): a medley tagged `hits` and `hits80` says `hits`'s name in its liners on both.
- The stations' codes show as badges in the medley list and on its page; the list can be filtered by station.
- Untagging a station is refused while the medley waits in that station's queue; its track leaves that station's rendered medleys' category.

## Roles

The roles ([API – Authentication](api.md#authentication)) are given **per station**, plus one for what is shared:

- **Shared role** (`viewer`, `operator`, `admin`): the library, artists, categories, medleys, shared settings, encoder profiles; `admin` also manages stations, users and their roles. The last enabled admin cannot lose it.
- **Role on each station** (`none`, `viewer`, `operator`, `admin`): its programming, queue, playout, outputs, webhooks and settings. With `none`, the user does not see the station.
- A shared `admin` is `admin` on every station.

An existing user keeps their role, as their shared role and their role on the first station. A user added without roles per station gets their shared role on every station; a new station is seen by shared admins only until roles are given on it.

- What a request may do follows the role on its station for the routes of a station (`/stations/{id}/…`, and the single-station routes for the first station), the shared role for the others. A station the user has `none` on answers `404`, and `GET /stations` leaves it out.
- `GET`, `POST /users` and `PATCH /users/{id}` carry `station_roles` (`[{"station_id", "role"}]`); a station left out is `none`. `GET /auth/me` has the user's own.

## Engines

**One engine per station.** Each has its own queue, decks, outputs, state volume and relay, and is upgraded with its own [hot handover](handover.md). A station's engine plays on with what it has queued when the API stops, as today.

- The API reads each station's engine address from its environment: `HERON_ENGINE_URL` for the first station, as today, and `HERON_ENGINE_URL_<CODE>` for each other one, its code in capitals (`HERON_ENGINE_URL_HITS80` for `hits80`). It runs one queue manager per enabled station. A station whose variable is not set stays off the air; the Stations page says which variable it waits for. Two stations never share an engine: a station whose address is already the engine of another running station (the same address, a trailing `/` and the letter case aside) stays off the air too, and the Stations page names the station that has it. The station that started first keeps it: at start, the first station, then the others by creation. The environment is read when `heron-api` starts: Compose starts it again when its variables change.
- A new station's engine is a new service of the deployment, with its own state volume: with Docker Compose, a second `heron-engine` service in a `docker-compose.override.yml`, then `docker compose up -d`. [Upgrading](upgrading.md) upgrades each engine in turn.
- Metrics carry the station's code as a label.

## API

- What is per station lives under **`/api/v1/stations/{id}/…`**: queue, player, deck controls, history, clocks, schedule, restrictions, rotation and its simulations, selection log, playout, EQ, queue and rotation settings, outputs, webhooks, listener statistics.
- `GET`, `POST /stations`; `GET`, `PUT`, `DELETE /stations/{id}` (admins).
- Shared resources keep their routes: tracks, artists, categories (with `station_ids`), medleys (with `station_ids`), labels, encoder profiles, users (with their roles per station).
- Track and artist answers carry their plays and last play per station and in total.
- **Public endpoints** per station: `/api/v1/public/stations/{code}/now-playing`, `/queue`, `/history`.
- **Webhooks** belong to a station; their deliveries carry the station's code.
- **Live WebSocket:** one per station, `/api/v1/stations/{id}/live` (`/api/v1/live` for the first station).
- **Compatibility:** the routes of a single station (`/queue`, `/player`, `/public/now-playing`…) keep answering, for the first station, until the next major version, so existing scripts and station websites keep working.

## Web UI

- A **station switcher** at the top of the menu, when there is more than one station the user can see: the On air, Queue, History, programming and station pages show the chosen station, kept per user. Library pages are shared and show the station badges.
- **Stations** page (admins): create, rename, enable or disable, and see each station's engine variable and whether it is set.
- **Users**: a role per station.
- Track and artist pages show plays per station and in total.
- A category created on the spot, from a track, a selection of tracks or the buffer, belongs to the station shown; the Categories page changes its stations.

## Limitations

- A track's categories, weights, cue points and overrides are the same on every station.
- A simulation simulates one station: the other stations' plays and queues are taken as they are.
- Moving categories or medleys between stations in bulk is not planned.
