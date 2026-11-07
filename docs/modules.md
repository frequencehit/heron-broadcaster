# Modules

Not every station wants every feature. Parts of Heron are **modules** that can be turned off: a module that is off is hidden and stopped, and its data is kept for when it is turned on again.

## The modules

| Module | Key | On after an upgrade | What it is |
|---|---|---|---|
| **Medleys** | `medleys` | yes | [Medleys](medleys.md): the editor, renders, and medley tracks on air |
| **Cover art** | `cover_art` | yes | Covers kept in Heron, their lookups on the Cover Art Archive, the station logo ([Library management – Cover art](library.md#cover-art)) |
| **Listener statistics** | `listener_statistics` | yes | Listeners read from the stream servers ([Listener statistics](statistics.md)) |
| **Webhooks** | `webhooks` | yes | [Webhooks](api.md#webhooks) on track changes |
| **Restrictions** | `restrictions` | yes | [Restrictions](rotation.md#restrictions): the hard rules of rotation |
| **Equalizer** | `eq` | yes | The [deck EQ](audio-engine.md#deck-eq) of the decks and the FX player, and its presets |
| **Processing** | `processing` | yes | The [master AGC chain](audio-engine.md#master-agc-chain) and its presets |
| **Trash** | `trash` | yes | The [trash](library.md#deleting-tracks-and-the-trash) of deleted tracks: the Trash page, restoring and emptying it. Installation only |
| **To do** | `review` | yes | The *To do* page: identifications to check after an import, the songs missing an intro, a hook or a cover, covers to check, organizing the library and undoing batches of file changes. Installation only |
| **Artists** | `artists` | yes | The Artists page: names, merges, similar artists and their suggestions. Installation only |
| **Labels** | `labels` | yes | [Labels](data-model.md#track-details-and-labels) on tracks: their page, filters, column and actions. Installation only |
| **Dedications** | `dedications` | no | A [show](listener-interaction.md): listeners ask for a song with a dedication, which an operator accepts into the queue |
| **Match** | `match` | no | A [show](listener-interaction.md): listeners choose the next song between two (#132) |
| **Top 30** | `top30` | no | A [show](listener-interaction.md#top-30): the weekly chart of the most liked tracks, played on air |
| **Likes** | `likes` | yes | A [show](listener-interaction.md#likes) without hours: listeners like or dislike the track playing |

The shows are off until an admin turns them on for the installation; then they are on for every station that does not turn them off. Likes are the exception: on by default.

## Two levels

- **The installation** (admins, *Administration › Modules*): a module turned off here is off for every station.
- **A station** (its admins, *Settings › Station › Modules*): a module on for the installation can be turned off for that station only.

Trash, To do, Artists and Labels are about the library, which the stations share: they are turned on and off for the installation only. A station's switch for them is ignored, and a station's modules page does not show them.

A module is on for a station when it is on at both levels. Nothing is stored until a module is changed: what is not stored takes its default, on for the installation except the shows (but Likes), and on for a station (a station only turns off what the installation has on). The installation's switches are the `modules` settings, a station's are its `station_modules` settings; [backups](backups.md) keep both, and a restore brings them back with the rest.

## Turned off

A module that is off for a station:

- **is hidden**: its menu entries, pages, settings and widgets disappear from the web interface, for that station; a link to one of its pages says that the module is turned off, and where to turn it on;
- **answers `404`** on its routes, with the error `module_off`: the routes of the station (`/stations/{id}/…`) when it is off for that station, every route of the module when it is off for the installation;
- **does nothing in the background**: nothing of it plays or is planned, nothing is fetched or sent;
- **keeps its data**: medleys, covers, statistics, webhooks and their deliveries stay in the database (cover images in the covers folder) and in backups, and are there again, as they were, once it is turned on.

What each module stops:

| Module | Off for a station | Off for the installation, also |
|---|---|---|
| **Medleys** | Rotation never picks a medley track (autofill, clocks, hours without a clock, simulations). A clock slot calling the station's medley category is passed over, and the rotation log says why (*module turned off*). A medley cannot be queued or played on the FX player by hand (`422`), and one already queued leaves the queue. `/settings/medleys` answers `404` | `/medleys/…` answers `404`: no edits, previews or renders. Medleys are shared by the stations, so their page is hidden for a station that turned them off, but their routes answer while the installation has them on |
| **Cover art** | `cover_url` is `null` in its history, its public endpoints and its webhooks; its logo (`/settings/station/logo`) answers `404`. Its pages show no covers | `/tracks/{id}/cover`, `/covers/…` and `/public/covers/{hash}` answer `404`, `cover_url` is `null` in the track list. `heron-librarian` looks nothing up on the Cover Art Archive, in the background or asked for; a lookup already queued fails, saying why. The scanner still reads the covers embedded in the files or beside them, which fetches nothing, so the covers are up to date when the module is on again |
| **Listener statistics** | Its stream servers are not read, and `/listeners/…` answers `404`. The status bar, On air and the Outputs page show no listeners | `/settings/statistics` answers `404`. Sessions and samples older than the retention are still deleted: it is a privacy rule |
| **Shows** (Dedications, Match, Top 30) | The show does not open, whatever its hours: the gateway answers `404` on its routes and its widget shows nothing; no `show.*` notification is sent. `/shows/{module}` answers `404`, and `GET /shows` leaves it out. The Top 30 freezes no chart, builds and plays no show (the clocks play its hours), sends no `top.*` notification, and `/top30/…` answers `404`: the charts already kept stay. Dedications take no request, send no `dedication.*` notification and show no dedication with the track playing; `/dedications/…` answers `404`, and the requests kept stay until their retention ends | The same for every station |
| **Likes** | Nothing can be voted on: the gateway answers `404` on its routes and its widget shows nothing. `/likes/…` answers `404`; the track list still gives the votes already counted, but the web interface hides them | The same for every station. Votes kept per device are still deleted on time: it is a privacy rule |
| **Webhooks** | No delivery is queued, and the deliveries still pending fail when they are due (*webhooks are turned off*), so that no old track change is sent once they are on again. `/webhooks/…` and `/webhook-deliveries/…` answer `404` | The same for every station |
| **Restrictions** | Rotation applies none of the restrictions on that station (autofill, clocks, simulations): they are not applied, not only hidden. Restrictions are shared by the stations, so their page is hidden for that station, but their routes answer while the installation has them on | `/restrictions/…` answers `404`, and no station applies them |
| **Equalizer** | The engine gets the station's deck EQ with both EQs bypassed: they glide to flat, as when an EQ is switched off, without a click. The gains are kept, and pushed again as they were once it is on. `/settings/eq` answers `404` | `/eq-presets/…` answers `404` |
| **Processing** | The engine gets the station's AGC chain bypassed (`bypass_all`): it crossfades out over 50 ms, as when the chain is switched off by hand. The chain's settings are kept, and pushed again as they were once it is on. `/settings/agc` answers `404` | `/agc-presets/…` answers `404` |

Off for the installation, the library modules:

| Module | Off for the installation |
|---|---|
| **Trash** | `/library/trash/…` answers `404`: the trash is not shown, restored or emptied. Deleting tracks still moves them to the trash, and it is still emptied after its retention (*Settings › Library*): turning the module off does not make deletions final at once, and the trash does not grow forever. A request to empty it queued before fails |
| **To do** | `/library/todo`, `/library/review`, `/tracks/{id}/review/…`, `/library/organize/…` and `/library/batches/…` answer `404`, and a review or an organize queued before fails. Imports still identify the tracks: those with an unsure match wait, as before, and are there once the module is on. The library keeps its journal. `GET /tracks` still filters the songs missing an intro, a hook or a cover. The covers to check are the cover art module's: their routes still answer while it is on, but their place on the page is hidden |
| **Artists** | `/artists/{id}/…`, `/artist-names/…` and `/artist-suggestions/…` answer `404`, and the MusicBrainz relationships are not looked up (a lookup queued before fails). `GET /artists`, the search of the artist pickers (restrictions, filters), still answers. What is set is still applied: merged artists stay merged, and rotation still keeps similar artists apart |
| **Labels** | `/labels/…` and `/tracks/labels` answer `404`. Labels are hidden everywhere: their page, the track list's column, filters and *Labels* action, the import buffer's, a track's labels, *Fill from a label* on the Categories page, and the label of a medley's recipe. Nothing in rotation uses labels, so nothing on air changes; a medley whose recipe has a label still draws its songs from it when it is rendered. The tracks keep their labels, and the API still gives them (`labels` of a track) and filters on them (`labels_any`, `labels_all`, `labels_none` of `GET /tracks`); a migration from another program still creates its labels |

## API

- `GET /modules` (signed in): the installation's switches, `{"medleys": true, "cover_art": true, "listener_statistics": true, "webhooks": true, "restrictions": true, "eq": true, "processing": true, "trash": true, "review": true, "artists": true, "labels": true, "dedications": false, "match": false, "top30": false, "likes": true}`.
- `PUT /modules` (admins): replaces them; a module left out takes its default (off for a show, on for Likes).
- `GET /stations/{id}/modules` (a role on the station): `installation` (as above), `station` (the station's own switches) and `on`, what is on for the station: the web interface reads it to show only what answers.
- `PUT /stations/{id}/modules` (admins of the station): replaces the station's switches; a module left out is on. A module off for the installation can be on here: it stays off until the installation turns it on. The library modules (trash, review, artists, labels) are stored on for a station, whatever is sent: only the installation turns them off.
