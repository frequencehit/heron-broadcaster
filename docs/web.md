# Web UI

`heron-web` serves the web app (React, Vite, TypeScript) and proxies `/api` to `heron-api`, WebSocket included: open `http://<host>:8000/` and sign in. Built in [M5](roadmap/m5-web-ui.md); the programming pages came with [M6](roadmap/m6-rotation-clockwheels.md), the translations with [M5d](roadmap/m5d-web-i18n.md). Everything it does goes through the [API](api.md), so scripts can do the same.

## Pages

| Page | What it does | Changes need |
|---|---|---|
| **On air** | Decks A and B (track, its categories with their color, progress, remaining time, which is on air, an `EQ` badge while their EQ is on and not flat, linking to the Deck EQ page), master and deck levels, skip, pause and resume, the FX player (play a jingle, liner or voice track over the music: it opens on the categories for the FX player, the last one used first, with the whole library as a last tab), the next tracks (with their categories), outputs and their state and listeners, the engine's state | operator |
| **Queue** | The waiting tracks, the cued one first, with their categories (color and name) and expected start times; drag to reorder (or use the keyboard: focus the handle, space, arrows, space), play next, remove, clear; add tracks from the library; autofill on or off | operator |
| **History** | Every play by day, with how it ended (played, skipped…) and for how long | |
| **Tracks** | The library: search, filters (playable, not analyzed, silent, missing, disabled, needs review), category, labels (any, all, none of), never played, not played for N days, added in the last N days, bitrate below, era, mood, sound code, the tracks of one import (from the Buffer page); sort by any column, either way; choose the columns (added, last played, plays, bitrate, year, era, BPM, mood, sound code, genre, labels); select tracks, a page or every matching track, then queue them, label them, put them in a category (or a new one, created on the spot), set their mood, sound code or BPM, edit their tags, go back to the files' tags, disable or enable them; delete a selection (to the trash, after a confirmation that lists what goes). The most used actions are buttons (play next, add to queue, edit tags, clear), the others in a **…** menu (keyboard: Enter or Space opens it, the arrows move, Escape closes); preview a track in the browser, from its cue-in to its cue-out, one at a time (every role; files the browser cannot play show a warning; `usePreview` in `web/src/lib/preview.ts` serves other lists too) | operator |
| **Buffer** | Upload files or folders (dragged, several at once, in resumable chunks, with each file's progress); the new files waiting to be imported, by group: tags as read and as they will be written, cover, planned path, identification and candidate releases, flags (duplicates, unreadable, too short); listen; pick a release or keep the file's tags; edit the tags of a file or a selection (title, artist, album, album artist, year, track, disc; edited fields are marked and can be undone); prepare a selection or a group for its tracks (categories, labels, mood, sound code, shown on each file and summed up by group; a missing category or label can be created on the spot); import or discard a selection, then follow the link to the tracks just imported. Clean files are selected already | operator |
| **Trash** | Deleted tracks, when they are removed for good; restore a deletion; empty the trash | operator (restore), admin (empty) |
| **Track** | Waveform with the cue-in, cue-out, where the next track starts (always shown: on the cue-out when the next track follows back to back; dragged or typed like the cue points) and the fades; a transition tried with another track, before or after this one, with the changes not saved yet; listen from anywhere, from the cue-in or the ending; drag the points or type them; Gap Killer and crossfade overrides; categories and weights, in a new category created on the spot if needed; labels, era, BPM, mood, sound code and the MusicBrainz details; analysis, file (codec, bitrate, sample rate, channels) and identification; plays; edit its tags; delete it | operator |
| **Artists** | Artists and their names (search any name); rename, move a name to another artist or split it off, merge artists, mark artists similar, restrict an artist; the suggestions to review, and the MusicBrainz relationships lookup | operator |
| **Clocks** | Each clock's slots, reordered by dragging (or with the keyboard), with their category, selection method, enforce-rules flag and separation overrides; whether the clock loops across hours; the hour (or the loop) drawn as it will sound, categories running short, an hour simulated | operator |
| **Schedule** | The schedules (see [Schedules and periods](#schedules-and-periods)); for the one picked, the week, hour by hour, in station time: pick a clock, then paint hours, rectangles, whole days, an hour of every day or the whole week, optionally only the empty hours, or copy a day onto others (see [Painting the schedule](#painting-the-schedule)) | operator |
| **Labels** | Create, rename, recolor, merge and delete labels, with their tracks | operator |
| **Categories** | Create, rename, recolor and delete categories; the stations they are used on; their type (Song, Jingle, Ad, or the station's own, managed on the same page); the separation windows they override; whether the FX player offers them (liners, power intros, voice tracks); fill a category from a label | operator |
| **Restrictions** | Dayparting, date ranges and sequences on tracks, artists and categories, on every station or some; turn them on and off | operator |
| **Selection log** | Each track rotation picked: clock and slot, rules held or relaxed, and the details (windows, tracks held back, candidates); a simulation of the next day or week, checked rule by rule | operator (simulations) |
| **Import** | Import another software's export: categories and problems named in Heron, conflicts settled, applied as one import; past imports, with undo ([Importing from another software](import.md)) | admin |
| **Review & organize** | Identifications waiting for review (apply a candidate release, or keep the file's tags); organize the library after a dry run; every batch of file changes (tag edits included), with undo | operator (review), admin (organize, undo) |
| **Crossfade & loudness** | Station crossfade, loudness and silence watchdog settings, with a live preview of the transition between two tracks (the one on air and the next by default) as you change them, which can be listened to | admin |
| **Deck EQ** | The music EQ (Decks A and B) and the FX EQ: ten vertical sliders each (±12 dB, double-click for 0 dB, arrows to move by 0.5 dB) with a number above each to type a gain to the tenth of a decibel, enable / bypass, the response curve drawn as the sliders move; presets: load into either EQ, save either EQ as a new preset, update a preset from the music EQ, rename, delete. Changes are heard as the sliders move, and other open pages follow them | operator |
| **Outputs** | Outputs (Icecast for now) and encoder profiles (MP3 for now), with each output's live state: connected, reconnecting and why, bytes sent, listeners; the stream server's admin credentials, for listener sessions | admin |
| **Statistics** | Listeners today, minute by minute; by day, week or month, per output or for the whole station: distinct listeners, peak, average, listening hours, sessions; the hours of the day; players; tracks with the most departures; CSV downloads ([Listener statistics](statistics.md)) | |
| **Settings** | Autofill, rotation (separation windows, fallback category, weights, rules across stations), the station's time zone, programming (what counts as a play, eras, mood and sound code scales) and the MusicBrainz details lookup, library layout and identification (AcoustID key), upload limits and how long the trash keeps tracks, analysis thresholds, listener statistics (minimum session, how long sessions are kept), medleys (where rendered medleys go, the liners, naming, the defaults of a new medley) | operator (queue, rotation, programming, medleys), admin (others) |
| **Webhooks** | Webhooks on track changes: address, events, FX or not, the default JSON body or a template; the secret, shown once (and a new one); a test ping; each webhook's deliveries, with their status, attempts, what was sent and answered, and send again | admin |
| **Stations** | The stations: add one with the address of its engine, rename, change its code, its engine or its position, enable or disable, delete ([Several stations](stations.md)) | admin |
| **Users** | Users and roles, passwords, enable or disable; with several stations, a shared role and a role on each station | admin |
| **Account** | Theme, language, width, password, API tokens | |

Viewers see every page (except users and webhooks) without the controls. Every page shows a banner when something needs attention: the API, the database or the engine unreachable, live updates disconnected, the engine refusing the configuration, the web UI and the API **not from the same version** (an image pulled without the other: pull and start both again), or the **server clock** not synchronized or off by more than about a second (from `GET /api/v1/status`, see [API](api.md#authentication)).

## Several stations

With [several stations](stations.md#web-ui), a **station switcher** at the top of the menu chooses the station the pages show, among those the user sees; the choice is kept in the browser, per user. The pages of a station (On air, Queue, History, Statistics, the programming and station pages) show that station, and their actions follow the user's role there; the library pages are shared and show each category's, medley's and restriction's stations as badges. The track and artist pages give the plays per station, and the track list can count the plays of one station. With one station, nothing changes: no switcher, no badges.

## Painting the schedule

On *Schedule*, pick a clock (or *No clock*), then:

- **click** an hour to paint it; **drag** from one hour to another to paint the rectangle between them (Monday to Friday, 06:00 to 10:00); **Shift + click** paints the rectangle from the hour clicked last;
- click a **day** (the row's name) to paint its 24 hours, or an **hour** (the column's number) to paint that hour on the 7 days;
- **Mon–Fri**, **Sat–Sun** and **The whole week** paint those days whole;
- **Only the empty hours** keeps the hours that already have a clock, whatever is painted (a default clock everywhere, the special shows untouched);
- **Copy a day** puts a day's hours onto the days ticked (Monday onto Tuesday to Friday).

**Undo** (Ctrl/⌘ + Z) takes back the last stroke; nothing is saved until **Save**, and **Revert** goes back to the saved schedule. The day and hour headers are buttons, usable with the keyboard, and say what they paint (*Paint Monday with Morning drive*).

## Schedules and periods

*Schedule* opens on the schedule playing now. Above the week:

- a **schedule picker**: the default schedule first, then the others by name, each with its periods;
- what plays **now** and the **next change** (*Christmas from 1 December 00:00*);
- **New schedule**: a name, and *Start from*: empty, or a copy of another schedule's hours;
- **Rename** and **Delete** (not for the default schedule; deleting asks first).

A schedule other than the default lists its **periods**: from a date and hour to a date and hour (the end excluded, in station time), and **Every year**. A period is added, changed or removed in the list and saved with the week by **Save**. A schedule with no period never plays, and says so.

## Live updates

The app keeps one WebSocket to `/api/v1/live` ([API – Live WebSocket](api.md#live-websocket)): the decks, queue and outputs change as soon as the engine reports them, positions move smoothly between the twice-a-second updates, and levels arrive ten times a second. It reconnects by itself (from half a second up to 10 s between tries). Changes are sent with REST calls; the queue shows a drag at once and follows the engine's answer.

## Selecting rows

In the lists with selection checkboxes (*Tracks*, *Buffer*):

- a **click** on a checkbox checks or unchecks its row, and makes it the **anchor**;
- **Shift + click** on a checkbox gives every row from the anchor to this one, both included, the new state of the checkbox clicked: check 8, then Shift + click 12, and rows 8 to 12 are checked; uncheck 9, then Shift + click 11, and 9 to 11 are unchecked. The anchor stays, so another Shift + click changes the range;
- **Ctrl + click** (⌘ + click on macOS) **anywhere on a row** checks or unchecks it without opening it, and makes it the anchor; a plain click on a row still opens it.

The range follows the rows as they are shown (sort, filters, page; in the Buffer, every group in turn). Another page or other filters drop the anchor.

## Waveform and previews

The waveform comes with the analysis ([Import and analysis](analysis.md)): one point per 50 ms, on a dB scale so quiet intros and fade-outs show. Tracks analyzed before M5 are analyzed again once to get theirs. Previews play the file itself (`GET /api/v1/tracks/{id}/audio`) in the browser, at its own level, without the station's processing; formats the browser cannot play (some FLAC or ALAC files in Safari) stay silent. The transition preview of *Crossfade & loudness* mixes its two files in the browser the way the engine plans it: the incoming track starts at its planned time, each track has its loudness gain and its fade, and a limiter keeps the sum from clipping. It plays from a few seconds before the transition to a few seconds after, and stops when the tracks or the settings change. It is a preview: the engine's own processing (and its audio processing, from M9) may sound slightly different.

While listening on a track's page, the playhead's time shows next to *Listen*, in the file's own timeline with tenths of a second (`0:20.9 / 4:04`, the file's length after it; its tooltip gives the time from the cue-in). It stays shown, frozen, when paused. Hovering the waveform shows the time under the pointer, so a point can be found before clicking (a click plays from there).

Points dragged on the waveform become the track's overrides once saved. *Next starts* is a point like the cue-in and cue-out: dragged on the waveform, or typed in the *Next starts* field next to them, on the file's timeline. Setting it makes a fixed crossfade for the track, that long before its cue-out (so it follows the cue-out if that moves); emptying the field goes back to the station's mode. It is always on the waveform: when the next track follows back to back (no crossfade point found by the analysis in automatic mode, or crossfading off for the track), it sits on the cue-out and says so (*Next starts, back to back*), and setting it turns crossfading back on for the track. A track of `min_track_ms` or shorter (a jingle) is followed back to back unless its point is set here; with a point, the next track overlaps it without fades ([Audio engine](audio-engine.md#transitions)).

**Try a transition** under the waveform plays this track into another, or another into this one: pick the other track and its side (*before* or *after*), then listen. It uses the station's crossfade and loudness settings and this track's points as they are on the page, saved or not, so a point can be moved and heard again before saving; the station hears nothing. The *Intro ends* flag (where the vocals start, for [jingles over the intro](audio-engine.md#jingles-over-the-intro)) is dragged the same way, or typed in the *Playout* card, and saved with it; emptying the field removes it. The **hook** (the song's most recognizable part, used for medleys) is set the same way: *Hook in* and *Hook out* flags, or the card's *Hook* fields, both empty for none. **Loop the hook** plays from hook in to hook out over and over, with the points being edited, until **Stop**. Queued entries of the track follow within seconds.

## Times

Every time and date is shown in the station's time zone (`station` settings, see [Rotation](rotation.md)), whatever the browser's: the history's days, the queue's start times, the selection log and the schedule agree with the clocks and restrictions. The History and Schedule pages name the zone.

## Themes

Light and dark themes. *System* (the default) follows the device's setting (`prefers-color-scheme`); *Light* or *Dark* is saved per user by the API (`PUT /api/v1/auth/preferences`) and also remembered by the browser, so the first paint uses it.

Pages are a centred column by default. *Use the whole width of the screen* (Account › Display) drops that limit, for wide screens and the many columns of the track list; it is saved and remembered like the theme (`{"full_width": true}`).

## Languages

English and French. *System* (the default) follows the browser's languages (`navigator.languages`: the first one the web UI has, English otherwise); a fixed language is saved per user by the API (`PUT /api/v1/auth/preferences`, `{"language": "fr"}`) and remembered by the browser, like the theme, so the sign-in page and the first paint already use it. `<html lang>` follows.

Dates, times, numbers, sizes, levels, relative times and plurals are written the language's way (`-1,5 dB`, *il y a 5 minutes*, *2 pistes*), with the browser's own variant of it when it has one (`en-GB` writes 24-hour times). Duration fields take a comma as well as a dot (`1:23,5`). Only the language changes, never the time zone (see [Times](#times)).

Not translated: what the station's people wrote or the files carry (tags, artist, category, clock and output names), the messages the API writes (the web translates the error codes it knows and shows the others' message as it is), logs, the command-line tools and the documentation.

### Adding a language

1. Copy `web/src/i18n/fr.ts` to `web/src/i18n/<code>.ts` and translate it. Messages are keyed by id; `{name}` is a parameter, which a translation can move but not rename; plurals are objects of [`Intl.PluralRules` forms](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html) (`one`, `few`, `many`, `other`…) chosen by `{count}`, `other` being required.
2. Add a line to `LANGUAGES` in `web/src/i18n/index.ts`, with the language's name in that language.
3. `npm run typecheck` fails on a missing or misspelled key, and `npm test` (`catalogs.test.ts`) on a parameter that differs from English.
4. Check the pages with the pseudo-locale (below), then in the language at phone width.

Catalogs other than English are loaded only when chosen. The API takes any language tag, so adding one needs no change there.

### Pseudo-locale

In development, `?lang=pseudo` on any page shows English accented, a third longer and in brackets (`[Šàṽé ··]`): text left out of the catalogs stands out, and so do layouts that break with longer words. It lasts until the page is reloaded without it, and is never saved.

## Layout

The layout works from phone width up: below 760 px the navigation moves to a menu, tables drop their secondary columns and cards stack.

### Breadcrumb

Above the title of every page that belongs to a section of the navigation, a breadcrumb says where it sits: the section (plain text, it has no page of its own), then the page, for instance `Library › Tracks` or `Station › Settings`. Pages outside a section (On air, Queue, History, Statistics, Account) have none.

A page inside a page adds its own name, last and not a link: `Library › Tracks › Space Cowboy` on a track, `Programming › Clocks › Morning drive` (or `New clock`) on a clock, `Library › Medleys › …` on a medley. The link back to the list brings it back as it was left: the same search, filters, sort and page (they are in its address, remembered while the app stays open).

The breadcrumb is built from the navigation's own list of pages, so a page added there gets one. It is a `nav` named *Breadcrumb*, its last item marked as the current page. Below 760 px, only the level above the page shows, as a back link (`‹ Tracks`), on pages inside a page.

## Development

```sh
cd web
npm ci
HERON_API_URL=http://localhost:8000 npm run dev   # against a running station's heron-web
npm test                                          # unit tests (Vitest), catalogs included
npm run typecheck && npm run build
```

Open a page with `?lang=pseudo` to check translations ([Pseudo-locale](#pseudo-locale)). `HERON_API_URL` defaults to `http://localhost:8080`, a `heron-api` run with `cargo run -p heron-api`. The API client is typed from the OpenAPI document ([API – Web client](api.md#web-client)). The app uses TanStack Query for requests and dnd-kit for dragging; routing is a small module on the History API (`src/lib/router.tsx`), since nginx answers every path with `index.html`.

## Limitations

- The FX player and transport act at once; there is no undo for a skip.
- Only Icecast outputs and MP3 profiles have forms: the other kinds arrive with their encoders and sinks in [M11](roadmap/m11-sinks-live-dj.md) (the engine refuses them until then).
- An upload resumes after a dropped connection, but not after the page is reloaded. Covers in the buffer are loaded by the browser from the Cover Art Archive.
- The clock editor draws each slot with its category's average length: real tracks vary, so the hour as it sounds drifts from the drawing; simulate it to see real picks.
