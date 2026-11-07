# Web UI

`heron-web` serves the web app (React, Vite, TypeScript) and proxies `/api` to `heron-api`, WebSocket included: open `http://<host>:8000/` and sign in. Built in [M5](roadmap/m5-web-ui.md); the programming pages came with [M6](roadmap/m6-rotation-clockwheels.md), the translations with [M5d](roadmap/m5d-web-i18n.md). Everything it does goes through the [API](api.md), so scripts can do the same.

## Pages

| Page | What it does | Changes need |
|---|---|---|
| **On air** | The listeners' [likes](listener-interaction.md#likes) on the track playing; the [dedication](listener-interaction.md#dedications) of the track playing, to read on air if a host is there; the [match](listener-interaction.md#match) in progress, as on its page, to close its vote or cancel it; decks A and B (track and its cover, its categories with their color, progress, remaining time, which is on air, an `EQ` badge while their EQ is on and not flat, linking to the Deck EQ page), master and deck levels, skip, pause and resume, the FX player (play a jingle, liner or voice track over the music: it opens on the categories for the FX player, the last one used first, with the whole library as a last tab), the next tracks (with their cover and categories: the first six, with a link to the Queue page; or the whole queue, to reorder, play next and remove there as on the Queue page, with *Show the whole queue on On air*, Account › Display), outputs and their state and listeners, the engine's state | operator |
| **Queue** | The waiting tracks, the cued one first, with their cover, their categories (color and name) and expected start times (crossfades included: each track starts where the one before hands over to it); a jingle of a [linked slot](rotation.md#linked-slots) is drawn joined to its track, and moves with it; a track with an [exact time](rotation.md#exact-times) shows when the engine starts it at the latest on a colored background, with a clock, instead of an estimate (*10:00:00*, the end of the window in soft mode), and the tracks after it follow from that time, and the second part of a shortened medley says it continues the one before; drag to reorder (or use the keyboard: focus the handle, space, arrows, space), play next, remove, clear; add tracks from the library; autofill on or off | operator |
| **History** | Every play by day, with its cover, with how it ended (played, skipped…) and for how long; one track's plays on one day, from the track page | |
| **Tracks** | The library, with each track's cover: search on its own line, then the [station](#station-filter), category (or no category), play ratio, the share of [likes](listener-interaction.md#likes) (*70 % and more*, *under 40 %, to rest*, at least a number of votes) and the period of the likes (7, 30 or 365 days), and sort, each menu's name above it; under *More filters*: status (playable, not analyzed, silent, missing, disabled, needs review, no cover), daypart separation, labels (any, all, none of), never played, not played for N days, added in the last N days, bitrate below, era, mood, sound code, the play ratio filter (below 1, 0.1 to 0.3, disabled; on a station, its ratio there, a track disabled there counting as disabled), daypart separation (the track's own, or its categories'), the tracks of one import (from the Buffer page); sort by any column, either way; choose the columns (added, last played, plays, bitrate, year, era, BPM, mood, sound code, genre, labels, lowest and highest level, *Liked* (the share of likes, a bar of likes and dislikes, and the number of votes, *not ranked yet* below 10), the track's own daypart separation (its N and its days in the same daypart, *Category* when it follows its categories), play ratio: on a [station](#station-filter), the track's ratio there, marked *disabled on this station* when it is, and the default under *All stations*; hovering a disabled track says who disabled it and when); select tracks, a page or every matching track, then queue them, label them, put them in a category (or a new one, created on the spot), set their mood, sound code, BPM, own daypart separation (N and the days in the same daypart, each kept, set or back to the categories') or play ratio (on the station shown, or the default under *All stations*; 0 disables them, there or everywhere, after a confirmation), import play ratios from a CSV file (one column per station code, after a dry run listing the rows that match no track and the values that are not ratios), edit their tags, disable or enable them (enabling asks for one ratio for all), find their covers on the Cover Art Archive; delete a selection (to the trash, after a confirmation that lists what goes). The most used actions are buttons (play next, add to queue, edit tags, clear), the others in a **…** menu (keyboard: Enter or Space opens it, the arrows move, Escape closes); preview a track in the browser, from its cue-in to its cue-out, one at a time (every role; files the browser cannot play show a warning; `usePreview` in `web/src/lib/preview.ts` serves other lists too) | operator |
| **Import music** | Upload files or folders (dragged, several at once, in resumable chunks, with each file's progress); the new files waiting to be imported, by group: tags as read and as they will be written, cover, planned path, identification and candidate releases, flags (duplicates, unreadable, too short); listen; pick a release or keep the file's tags; edit the tags of a file or a selection (title, artist, album, album artist, year, track, disc; edited fields are marked and can be undone); prepare a selection or a group for its tracks (categories, labels, mood, sound code, shown on each file and summed up by group; a missing category or label can be created on the spot); import or discard a selection, then follow the link to the tracks just imported. Clean files are selected already | operator |
| **Trash** | Deleted tracks, when they are removed for good; restore a deletion; empty the trash | operator (restore), admin (empty) |
| **Track** | In tabs, under the track's name and actions: *Sound* (waveform, a transition to try, the playout overrides), *Programming* (categories, play ratio, daypart separation, programming details), *Details* (cover, analysis and file, identification) and *Plays* (recent plays, the listeners' opinion, plays by day and hour); the tab is in the address (`?tab=programming`). The listeners' opinion on the station shown: this week, 30 days and always, share of likes and votes; its latest plays with their likes and dislikes (see [Likes](listener-interaction.md#likes)). Waveform with the level in dBFS under the pointer and at the playhead, the lowest and highest level between the cue points, the cue-in, cue-out, where the next track starts (always shown: on the cue-out when the next track follows back to back; dragged or typed like the cue points) and the fades; a transition tried with another track, before or after this one, with the changes not saved yet; listen from anywhere, from the cue-in or the ending; drag the points or type them; Gap Killer and crossfade overrides; the play ratio: the default, then one line per station with its ratio there (the default, or its own; 0 disables the track on that station only, after a confirmation, and re-enabling asks for the ratio it had there), its counter drawn as its 10 steps (those that play and those held back, the next one framed) and reset to 10; who disabled the track and when, everywhere or on a station; weight and ratio explained side by side; its own [daypart separation](rotation.md#daypart-separation) and same daypart on the following days, each *the category's* or a value of its own, with what applies in each of its categories and each station's maximum; categories and weights, in a new category created on the spot if needed, with what a weight does; disable it (or give it a ratio of 0) after a confirmation, re-enable it at the ratio asked for (the one it had, or 1); labels, era, BPM, mood, sound code and the MusicBrainz details; analysis, file (codec, bitrate, sample rate, channels) and identification; its [cover](#cover-art); plays, and when they happened, by day and by hour of the week ([Plays by day and hour](#plays-by-day-and-hour)); edit its tags; delete it | operator |
| **Artists** | Artists and their names (search any name); rename, move a name to another artist or split it off, merge artists, mark artists similar, restrict an artist; the suggestions to review, and the MusicBrainz relationships lookup | operator |
| **Clocks** | Each change saves itself a moment later (*Saving…*, *Saved*, or *Not saved* with the reason, the change kept to fix); a new clock is created with *Create*; Each clock's slots, reordered by dragging (or with the keyboard), with their category, selection method, enforce-rules flag, *Linked to the next slot* flag (a jingle that plays only with the next slot's track, see [Linked slots](rotation.md#linked-slots)), its [exact time](rotation.md#exact-times) (a time in the hour, hard or soft, the soft window, *fill with a medley*, and *cut live* for M11: in a dialog, and shown on the slot as `:00:00 · hard`) and separation and [draw depth](rotation.md#draw-depth) overrides, with what the method, the weights, the ratio and the depth each do; whether the clock loops across hours; the hour (or the loop) drawn as it will sound, categories running short, an hour simulated | operator |
| **Schedule** | The schedules (see [Schedules and periods](#schedules-and-periods)); for the one picked, the week, hour by hour, in station time: pick a clock, then paint hours, rectangles, whole days, an hour of every day or the whole week, optionally only the empty hours, or copy a day onto others (see [Painting the schedule](#painting-the-schedule)) | operator |
| **Labels** | Create, rename, recolor, merge and delete labels, with their tracks | operator |
| **Categories** | Create, rename, recolor and delete categories; the stations they are used on, and those of one [station](#station-filter); their type (Song, Jingle, Ad, or the station's own, managed on the same page); the separation windows they override, and their daypart separation (0: off) and same daypart on the following days (0 to 7 days), shown in the list; their [draw depth](rotation.md#draw-depth) (start and maximum, each the station's or their own); whether the FX player offers them (liners, power intros, voice tracks); fill a category from a label | operator |
| **Restrictions** | Dayparting, date ranges and sequences on tracks, artists and categories, on every station or some; turn them on and off | operator |
| **Rotation log** | Each track rotation picked: its daypart, clock and slot (and the slots passed over, with why: a linked jingle skipped because of its track), rules held or relaxed (with where the track last played and what daypart separation held back or allowed), the draw depth it was drawn at (`30 %, 30 tracks, 4 held back by the artist`), how an [exact time](rotation.md#exact-times) was met (`:00:00 hard · on time, the track before faded`, `early by 12 s`) and what filled the gap before it (a medley, with the hooks left out, or a filler track), the endings simulated before a pick near an exact time and how the best one ends (`10 endings simulated, the best ends at 59:56, no filling`, or that even the best one fades the track before out), the tracks held back by their play ratio with their counter, the picked track's ratio on the station and its counter, and the details (windows, tracks held back, candidates); a simulation of the next day or week, checked rule by rule; reset every play ratio counter of the station to 10, after a confirmation | operator (simulations, reset) |
| **Migrate** | Bring in another software's export: categories and problems named in Heron, conflicts settled, applied as one import; past imports, with undo ([Importing from another software](import.md)) | admin |
| **To do** | What is left to do on the library, in tabs with their count (see [To do](#to-do)): *To identify* (identifications waiting for review: apply a candidate release, or keep the file's tags), *Missing intros*, *Missing hooks* and *Missing covers* (the songs without one; covers to check first: pick one of an album's candidates, or none), *Organize* (organize the library after a dry run) and *Changes* (every batch of file changes, tag edits included, with undo) | operator (review), admin (organize, undo) |
| **Dedications**, **Match**, **Top 30**, **Likes** | Under *Shows*, one page per [show](listener-interaction.md) turned on, see [Shows](#shows) | operator |
| **Crossfade & loudness** | Station crossfade, loudness and silence watchdog settings, with a live preview of the transition between two tracks (the one on air and the next by default) as you change them, which can be listened to | admin |
| **Deck EQ** | The music EQ (Decks A and B) and the FX EQ: ten vertical sliders each (±12 dB, double-click for 0 dB, arrows to move by 0.5 dB) with a number above each to type a gain to the tenth of a decibel, enable / bypass, the response curve drawn as the sliders move; presets: load into either EQ, save either EQ as a new preset, update a preset from the music EQ, rename, delete. Changes are heard as the sliders move, and other open pages follow them | operator |
| **Processing** | The station's [AGC chain](audio-engine.md#master-agc-chain): bypass all, and one panel per block in the chain's order (gated AGC, stereo expander, bass EQ, 5-band, dual-band, final stage), each with its switch, a line saying what it does, and its settings as sliders with a number input; the bands of a processor side by side, with their mode and the links between them. Live meters: the chain's input and output, the AGC's gain, each band's gain reduction and the final stage's. Presets: load, save the chain as a new preset, update a preset from the chain, rename, delete. Changes are heard as the controls move, and other open pages follow them | operator |
| **Outputs** | Outputs (Icecast for now), each with its stream server, mount, encoder profile and the station details it overrides, with its live state: connected, reconnecting and why, bytes sent, listeners; **Duplicate** (a disabled copy, its mount suffixed `-copy`); **Servers**: the station's stream servers (address, source and admin credentials, public address) and relays (their master, the mounts they relay); encoder profiles | admin |
| **Statistics** | For the whole station or one output, all servers or one and over the last 7, 30, 90 or 365 days (the filters are for the whole page, kept in the address: `?output=`, `?server=`, `?days=`), as an audience report: the audience figures as tiles (total listening time (TTSL), average listening time (ATSL) and median, unique listeners (cume), average quarter-hour audience (AQH), peak audience, sessions), each with its change against the previous period and an info button explaining it, and the session lengths; the trend by day, week or month; the hours of the day as a heatmap (day of the week by hour) with tune-in and tune-out; countries (with a GeoIP database), devices and players, as shares of the listeners and of TTSL; tracks with the most departures; the week's most and least liked tracks (with [likes](listener-interaction.md#likes) on); listeners today, minute by minute; *How these figures are counted* at the bottom of the page; CSV downloads. A user who sees several stations can add them up or compare them (`?stations=all`, `?stations=1,2`, `?stations=compare-all`): side-by-side tiles with the difference, lines per station, a table per station ([Listener statistics – Web UI](statistics.md#web-ui)) | |
| **Settings** | In tabs, the station's first: *Station* (time zone, public address, logo, station details, modules), *Programming* (autofill, rotation, programming details, MusicBrainz, medleys), *Audio* (analysis), *Library* and *Statistics* (with the listener statistics module); a tab is in the address (`/settings?tab=library`). Autofill, rotation in titled sections, each with a one-line explanation (separation, dayparts, draw depth with a diagram of a category of 100 tracks that follows the two values (drawn first, added when nothing fits, never drawn, played recently), when no track fits: fallback and filler categories, weight after a play, the station's library, across stations), rotation (separation windows, the station's dayparts with an example over several days and a link to the categories, where the separation is set, fallback category, filler category (for the gaps before [exact times](rotation.md#exact-times)), categories kept out of the station's library, weights, the draw depth (start and maximum), rules across stations), the station's time zone, public address, gateway address and logo, the station details sent to the stream servers (name, description, genre, website, public), programming (what counts as a play, eras, mood and sound code scales) and the MusicBrainz details lookup, library layout and identification (AcoustID key), the Cover Art Archive lookup of missing covers (in the background, or now), writing the tags kept in Heron into the files, upload limits and how long the trash keeps tracks, analysis thresholds, listener statistics (minimum session, how long sessions are kept), medleys (where rendered medleys go, the liners, naming, the defaults of a new medley), the station's [modules](#modules) | operator (queue, rotation, programming, medleys), admin (others) |
| **Webhooks** | Webhooks on track changes: address, events, FX or not, the default JSON body or a template; the secret, shown once (and a new one); a test ping; each webhook's deliveries, with their status, attempts, what was sent and answered, and send again | admin |
| **Stations** | The stations: add one with the address of its engine, rename, change its code, its engine or its position, enable or disable, delete ([Several stations](stations.md)) | admin |
| **Users** | Users and roles, passwords, enable or disable; with several stations, a shared role and a role on each station; whether each user has a second factor (on, with the app and how many passkeys; *to set up* when their role requires it and they have none; off), and which roles require one ([Two-factor authentication](#two-factor-authentication)) | admin |
| **Modules** | Under *Administration*: each [module](modules.md) (medleys, cover art, listener statistics, webhooks, restrictions, equalizer, processing, trash, to do, artists, labels, the shows), what it does and what turning it off stops, with its switch for the whole installation | admin |
| **Backups** | The backups, newest first (kind, time, the size of its two files, a failed dump), *Back up now*, download either file, restore a backup or an uploaded configuration file (with what it will replace, then its report; the key for an encrypted one), delete; the `backups` settings; encryption: encrypt backups (the key shown once, then typed back, or the admin's own public key), check the key, change it, turn encryption off, which backups need an earlier key, the public key each key typed must match ([Backups – Encryption](backups.md#encryption)); *Test* on a backup, the last test, damaged backups ([Testing backups](backups.md#testing-backups)) | admin |
| **Account** | Theme, language, width, the whole queue on On air, password, API tokens, [two-factor authentication](#two-factor-authentication) (the authenticator app, passkeys, recovery codes) | |
| **About** | The build (version, commit, each service's version and whether it answers, web UI and API from the same build or not, links); for admins also the runtime (uptime, operating system, container, time zone and clock, CPU and memory), the dependencies (database, FFmpeg and stream servers versions) and the active configuration, read-only, with secrets only as *set* or *not set*; *Copy diagnostics* puts it all in the clipboard for a bug report. Linked at the bottom of the navigation | admin (all but the build) |

Viewers see every page (except users and webhooks) without the controls. Pages, settings and widgets of a [module](#modules) turned off are not shown. Every page shows a banner when something needs attention: the API, the database or the engine unreachable, live updates disconnected, the engine refusing the configuration, the **backup folder** not writable (admins), **backups not encrypted**, **damaged** or **not tested** (admins), the web UI and the API **not from the same version** (an image pulled without the other: pull and start both again), or the **server clock** not synchronized or off by more than about a second (from `GET /api/v1/status`, see [API](api.md#authentication)). When an admin's [key check](backups.md#checking-the-key) is due, a dialog asks for the backup key, on any page.

## Modules

A [module](modules.md) turned off for the station shown disappears: its entries in the menu, its pages, its cards on the Settings page and its widgets elsewhere. The web interface reads what is on from `GET /stations/{id}/modules`, so it never shows a page that answers `404`.

| Module | Hidden when it is off |
|---|---|
| Medleys | The Medleys page, the medley settings |
| Cover art | Covers everywhere (On air, Queue, History, the track list with its column, *No cover* filter and *Find covers*, the track's page), the *Missing covers* tab of *To do*, the Cover Art Archive lookup in the library settings, the station logo |
| Listener statistics | The Statistics page, the listeners of the status bar, On air and the Outputs page (with the stream servers' admin credentials and relays), the statistics settings |
| Webhooks | The Webhooks page, the notifications of the shows' pages |
| Restrictions | The Restrictions page, the restrictions link and *Restrict* button of an artist |
| Equalizer | The EQ page, the *EQ* badge of the decks On air |
| Processing | The Processing page |
| Trash | The Trash page, its links from the track list and a track's page |
| To do | The To do page, a track's *needs review* banner, the link from the tags dialog |
| Artists | The Artists page, the links to it from the Restrictions page |
| Labels | The Labels page, the track list's labels column, filters and *Labels* action, the same in the import buffer, a track's labels, *Fill from a label* on the Categories page, the label of a medley's recipe |
| Dedications, Match, Top 30 | The show's page, and the *Shows* section when no show is on; for Dedications, the dedication on On air; for the Match, the match in progress on On air; for the Top 30, the track page's rank in the chart and the likes widget's line |
| Likes | Its page under *Shows*, the votes on On air, the track page's listeners' opinion, the *Liked* column and filters of the track list, the week's most liked tracks on Statistics |

An address of a page that is off (a bookmark, a link) shows *This module is turned off* instead, with a link to where an admin turns it on.

**Administration › Modules** (admins) turns a module on or off for the installation; **Settings › Station › Modules** (admins of the station) for the station shown. A module off for the installation shows there as off, and cannot be turned on for one station. The library modules (trash, to do, artists, labels) are only on the installation's page.

## Shows

The *Shows* section of the menu has a page per [show](listener-interaction.md) that is on for the station shown. Each page says when and how listeners take part, in the station's time zone, and holds the settings every show has; each show adds its own tabs (its requests, its match in progress) and settings.

- **Hours**: *Open during its hours* (`enabled`), and ranges of days and whole hours (*Mon Tue … Sun, from 17:00 to 18:00*), added and removed. A range that takes hours from another show says which, before saving. The week below draws the show's hours and, hatched, those of the other shows.
- **Notifications**: the station's webhooks that receive it, each event on or off with its message (and, for *Before opening*, how many minutes before), the language of the dates, and a preview of the messages as they will read, with an example. Hidden while webhooks are off.
- **On the website**: the line of the dedicated page and of the small widget, each with *Copy the line*, and *Small widget* on or off; *View the page alone*; outside the hours, a message (with `{day}` and `{hour}`) or nothing; the color, the title and the website's page of the show. A live preview, *Open* or *Closed*, follows the settings before they are saved. The lines start with the station's gateway address (*Settings › Station*); without one, the page says to set it.

**Top 30** has four tabs (see [Top 30](listener-interaction.md#top-30)):

- **Chart**: the chart of the week as it stands, live (rank, movement, the track, share of likes with its bar and votes, weeks in the chart), when it closes and the week's votes; *Take out* on each track, and the tracks taken out with who and when, to put back. A closed chart whose show has not started yet comes first, with the same controls.
- **The show**: the next show (or the one on air): how many tracks, the music, the jingles and the new release in time, against the slot; the new release, proposed or chosen among the tracks of its category; the programme in order with planned times, the ranks left out to fit the slot and the tracks passed over with why.
- **Past weeks**: the frozen charts, the latest first; one opens with its movements and the tracks taken out.
- **Settings**: the close of the week, the minimum votes, the size, the eligible categories, the jingle and new release categories; then the hours (the show's slot), the notifications and *On the website* every show has.

**Dedications** has two tabs (see [Dedications](listener-interaction.md#dedications)):

- **Requests**: whether the show is open and until when; the range's requests waiting, accepted and refused, and received against its maximum; then the requests by status, the latest first, updated every few seconds. A waiting request shows its track, *For Julie, from Marc*, the message, how long ago, and what the rules say (*rules kept*, *artist played 12 min ago (separation: 20 min)*, *already in the queue at 17:52*, *restriction: not before 20:00*), with *Accept next*, *Within the hour*, *Best place*, and *Refuse* with a reason. A request a restriction forbids cannot be accepted. Accepted ones show when they are planned or played.
- **Settings**: the hours; the requests (per range, per listener, the message's length, how long first names and messages are kept, the requestable categories); the notifications, with `{artist}`, `{title}`, `{from}`, `{to}`, `{message}` and `{time}`; and *On the website*, whose preview shows the form.

**Match** has three tabs (see [Match](listener-interaction.md#match)):

- **In progress**: the match queued or open, updated every second or two: its hours and *every 4 tracks*, the two tracks one against the other (category, artist and title, share of the votes as a bar, votes), and under them the vote's track as a line from its start to the next transition, with the announcement over its intro, now, the close of the vote and the transition. The countdown to the close, *Close the vote now* and *Cancel the match* (after a confirmation). Without a match: when the next one should come, or why none runs (closed, no category).
- **History**: the matches played, the latest first: when, the winner with its share (or *tie, least recently played*, *no vote*, *closed early*), the loser and the votes; cancelled ones marked so. The week's matches and votes above them.
- **Settings**: the hours; the match (every N tracks, the category of the two tracks, the announcement category, the seconds before the transition the vote closes, live results on the website, whether the loser can come back the same day); the notifications, with `{title1}`, `{artist1}`, `{title2}`, `{artist2}`, `{close}`, `{winner}`, `{loser}`, `{percent}` and `{votes}`; and *On the website*, with its name on the website (*The duel*, *Battle*, *You choose*: also in the public `now-playing` and the notifications), whose preview shows the vote.

**Likes** have no hours and send no notifications: their page has the week's most liked and least liked tracks of the station (share of likes, votes, plays; from 10 votes), then *On the website*. See [Likes](listener-interaction.md#likes).

Viewers see the settings without changing them.

## Several stations

With [several stations](stations.md#web-ui), a **station switcher** at the top of the menu chooses the station the pages show, among those the user sees; the choice is kept in the browser, per user. The pages of a station (On air, Queue, History, Statistics, the programming and station pages) show that station, and their actions follow the user's role there; the library pages are shared and show each category's, medley's and restriction's stations as badges. The track and artist pages give the plays per station, and the track list can count the plays of one station. The Statistics page can add up or compare the stations the user sees ([Listener statistics – Web UI](statistics.md#web-ui)). With one station, nothing changes: no switcher, no badges, no station filter. The pages call a station's routes under `/api/v1/stations/{id}/…`, typed from the OpenAPI document like every other call, so a page cannot reach another station's route by mistake.

### Station filter

The **Tracks**, **Categories** and **Medleys** pages show the switcher's station at first, and can show another station the user sees, or **All stations**. The choice is kept in the browser, per user and per page; a station the user no longer sees goes back to the switcher's.

- **Tracks:** a track is on a station when it is in one of the station's categories and in none it [keeps out of its library](rotation.md#clockwheel): what rotation can play there. Tracks without a category show only under *All stations*, and a station with no category yet shows none (its rotation plays the whole library until it has some); choosing *No category* in the category list shows them whatever the station. The category list offers the station's categories. The count, *every matching track* and the actions on a selection follow the filter.
- **Categories:** the categories the station uses.
- **Medleys:** the medleys tagged with the station.

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

In the lists with selection checkboxes (*Tracks*, *Import music*):

- a **click** on a checkbox checks or unchecks its row, and makes it the **anchor**;
- **Shift + click** on a checkbox gives every row from the anchor to this one, both included, the new state of the checkbox clicked: check 8, then Shift + click 12, and rows 8 to 12 are checked; uncheck 9, then Shift + click 11, and 9 to 11 are unchecked. The anchor stays, so another Shift + click changes the range;
- **Ctrl + click** (⌘ + click on macOS) **anywhere on a row** checks or unchecks it without opening it, and makes it the anchor; a plain click on a row still opens it.

The range follows the rows as they are shown (sort, filters, page; in Import music, every group in turn). Another page or other filters drop the anchor.

## Waveform and previews

The waveform comes with the analysis ([Import and analysis](analysis.md)): one point per 50 ms, on a dB scale so quiet intros and fade-outs show. Tracks analyzed before M5 are analyzed again once to get theirs. Previews play the file itself (`GET /api/v1/tracks/{id}/audio`) in the browser, at its own level, without the station's processing; formats the browser cannot play (some FLAC or ALAC files in Safari) stay silent. The transition preview of *Crossfade & loudness* mixes its two files in the browser the way the engine plans it: the incoming track starts at its planned time, each track has its loudness gain and its fade, and a limiter keeps the sum from clipping. It plays from a few seconds before the transition to a few seconds after, and stops when the tracks or the settings change. It is a preview: the engine's own processing (and its audio processing, from M9) may sound slightly different.

While listening on a track's page, the playhead's time shows next to *Listen*, in the file's own timeline with tenths of a second (`0:20.9 / 4:04`, the file's length after it; its tooltip gives the time from the cue-in). It stays shown, frozen, when paused. Hovering the waveform shows the time under the pointer, so a point can be found before clicking (a click plays from there).

Points dragged on the waveform become the track's overrides once saved. *Next starts* is a point like the cue-in and cue-out: dragged on the waveform, or typed in the *Next starts* field next to them, on the file's timeline. Setting it makes a fixed crossfade for the track, that long before its cue-out (so it follows the cue-out if that moves); emptying the field goes back to the station's mode. It is always on the waveform: when the next track follows back to back (no crossfade point found by the analysis in automatic mode, or crossfading off for the track), it sits on the cue-out and says so (*Next starts, back to back*), and setting it turns crossfading back on for the track. A track of `min_track_ms` or shorter (a jingle) is followed back to back unless its point is set here; with a point, the next track overlaps it without fades ([Audio engine](audio-engine.md#transitions)).

**Try a transition** under the waveform plays this track into another, or another into this one: pick the other track and its side (*before* or *after*), then listen. It uses the station's crossfade and loudness settings and this track's points as they are on the page, saved or not, so a point can be moved and heard again before saving; the station hears nothing. The *Intro ends* flag (where the vocals start, for [jingles over the intro](audio-engine.md#jingles-over-the-intro)) is dragged the same way, or typed in the *Playout* card, and saved with it; emptying the field removes it. The **hook** (the song's most recognizable part, used for medleys) is set the same way: *Hook in* and *Hook out* flags, or the card's *Hook* fields, both empty for none. **Loop the hook** plays from hook in to hook out over and over, with the points being edited, until **Stop**. Queued entries of the track follow within seconds.

## To do

*To do* (`/to-do`; the former address `/to-organize` leads there) gathers what is left to do on the library, a tab each, with its count in the tab:

- **To identify:** the identifications waiting for review, as after an import. Its count is also the menu's.
- **Missing intros:** the songs with no intro point (where the vocals start, for [jingles over the intro](audio-engine.md#jingles-over-the-intro)).
- **Missing hooks:** the songs with no hook.
- **Missing covers:** with the cover art module, the albums whose Cover Art Archive search was unsure (*covers to check*), then the songs with no cover.
- **Organize** and **Changes:** organizing the library after a dry run, and every batch of file changes with undo.

A **song** here is an enabled track whose file is there, in at least one category of the Song type (the one that exists from the start, whatever its name now): jingles, ads, liners and the tracks in no category are left out. The lists are library-wide, or of one song category chosen above them (kept from tab to tab, `?category=`), a page of 50 at a time, sorted by artist or title (and reversed); the counts in the tabs stay library-wide. Each track links to its page: the *Sound* tab, where the intro and the hook are set, or *Details* for its cover. Nothing is edited on *To do* itself.

## Cover art

Covers show wherever a track does: the track list (a thumbnail column), the track's page, On air, the queue and the history. A track without one shows a neutral placeholder. See [Library management – Cover art](library.md#cover-art).

On a track's page, the cover is shown with where it comes from (*set by hand*, *embedded in the file*, the file beside it, *Cover Art Archive*) and whether it is the album's or the track's own. Operators can change it for the **album** or for **this track only**:

- **Upload** an image (JPEG, PNG or WebP, up to 10 MB), or paste an **image address**;
- **Search the Cover Art Archive:** the results show with their cover, release, date and country; pick one;
- **Remove** it: the next source is looked for in the file and its folder.

In the track list, the filter *No cover* lists the tracks without one, and *Find covers* (in the **…** menu) looks the selection's albums up on the Cover Art Archive. An unsure search waits on *To do*, at the top of *Missing covers*: each album with its candidates, to pick one or refuse them all.

## Plays by day and hour

Two grids at the bottom of the track page say when the track played: every day or in bursts, in the morning or at night. They are built from the history ([API](api.md#plays-by-day-and-hour)).

- **Plays of the last 30 days, week by week:** a row per week, Monday to Sunday, with each day's number of plays and the week's total. The days before the 30 days and after today are greyed out, today is framed, and a day is darker the more it played. Hovering a day, or moving to it with the keyboard, gives the times of its plays, how each came (rotation, request, by hand) and its likes and dislikes; a click opens the history at that day, for that track.
- **Hours of the week:** the 24 hours by the 7 days of the week, a cell ticked when the track played in that hour on that weekday, darker the more it played. Hovering or focusing a cell gives the number of plays and their dates. Over 30, 90 or 365 days (30 by default).

Above both grids, the station (the one chosen in the switcher at first) or **All stations**, which adds the stations' plays up and says each play's station; and **All plays** or **Rotation only**. Times are in the station's time zone, or the first station's with all stations. When the clocks go back, the plays of both 02:00 hours are in the 02:00 row; when they go forward, that day's 02:00 row stays empty.

## Two-factor authentication

See [API – Two-factor authentication](api.md#two-factor-authentication) for how it works.

- **Account › Two-factor authentication:** set up an authenticator app (a QR code to scan, or the key to type, then a code it shows) or replace it; add passkeys, rename or remove them, with when each was last used; how many recovery codes are left, and *Generate new codes*. Setting up or removing a factor asks for your password, and signs you out everywhere else. When your role requires it, the last factor cannot be removed.
- **Recovery codes** are shown once, when the first factor is set up or new ones are generated: copy them, download them as a `.txt` file or print them, and tick *I have put these codes away* to close the dialog.
- **Signing in:** after the password, a user with a second factor types the 6-digit code of their app, or uses a passkey. *Use a recovery code* takes one of those instead, and *Change user* goes back to the password.
- **Required:** a user whose role requires it and who has none is asked to set one up right after the password, and sees nothing else until they have, then gets their recovery codes. *Sign out* leaves.
- **Users:** a *Two-factor authentication* card with one box per role (*Require for admins*, *operators*, *viewers*), and a column with each user's state. There is no reset button: a user who lost every factor is reset on the command line, `heron-api user reset-2fa <name>` ([API – Authentication](api.md#authentication)).
- Passkeys need HTTPS (or `localhost`) and a host name; the authenticator app works everywhere.

## Times

Every time and date is shown in the station's time zone (`station` settings, see [Rotation](rotation.md)), whatever the browser's: the history's days, the queue's start times, the rotation log and the schedule agree with the clocks and restrictions. The History and Schedule pages name the zone. When the clocks go back, the day has 25 hours and the repeated hour shows which it is, the way on-air logs write it: `02:40 (summer time)`, then `02:40 (winter time)`; the other times are shown as they are. When they go forward, the hour before and the hour after say it too: `01:59 (winter time)`, then `03:00 (summer time)`. Either way, the history has a line where the clocks changed, between the last play before it and the first one after: *Change to summer time · 02:00 → 03:00* on a yellow band, or *Change to winter time · 03:00 → 02:00* on a blue one.

## Themes

Light and dark themes. *System* (the default) follows the device's setting (`prefers-color-scheme`); *Light* or *Dark* is saved per user by the API (`PUT /api/v1/auth/preferences`) and also remembered by the browser, so the first paint uses it.

Pages are a centred column by default. *Width* (Account › Display) picks *Normal* (that column), *80 %* (80 % of the window's width, still centred, and never narrower than *Normal*) or *100 %* (the whole width), for wide screens and the many columns of the track list; it is saved and remembered like the theme (`{"width": "wide"}`).

*Show the whole queue on On air* (Account › Display, off by default) makes the On air page's *Up next* card list every waiting track instead of the first six, with the Queue page's controls: drag to reorder, play next, remove. The card then has no *Edit the queue* button; adding tracks stays on the Queue page. It is saved per user (`{"on_air_whole_queue": true}`).

## Languages

English, French, Spanish and German. *System* (the default) follows the browser's languages (`navigator.languages`: the first one the web UI has, English otherwise); a fixed language is saved per user by the API (`PUT /api/v1/auth/preferences`, `{"language": "fr"}`) and remembered by the browser, like the theme, so the sign-in page and the first paint already use it. `<html lang>` follows.

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

### Status bar

A line one character high stays at the bottom of every page, in the monospace font. From left to right:

- **Decks A and B:** the state (`▶` playing, `⏸` paused, `—` empty), then the artist and title with the elapsed time. The deck the listeners hear (on air) is brighter, with a red `▶`. Titles that do not fit are cut with an ellipsis. A line one pixel high along the bottom of each deck's part shows how far its track has played, in the deck's colour. A click on a deck opens the On air page. `engine ?` replaces both while the engine is unreachable.
- **Listeners:** the total of the current station, read like the [Statistics](statistics.md) page's: at most a minute old, and `–` when no server could be read (never `0`). A click opens the Statistics page.
- **System** (operators and admins): `cpu 12% mem 41% load 0.4 0.5 0.6`. The CPU and memory turn to the warning colour from 80 % and to the alert colour from 95 %. Viewers do not get this part.

While the live connection is down the bar greys out and says `offline`. Below 760 px the system part goes first, then the titles shrink to the deck letter and its state. It is a `status` region without live announcements: it changes too often to be read out, and each part carries a full label.

### About

The **About** link at the bottom of the navigation, under the account, opens a page with:

- **Build** (everyone): the web interface and each service (API, one engine per station, analyzer, librarian) with its version, commit, build date and whether it runs. A service that was never seen, or whose last sign of life is older than 90 seconds, shows as not running. The commit and the build date are set in the published images (`HERON_REVISION`, `HERON_BUILD_DATE`) and show `–` in a local build.
- **Runtime** (admins): how long each service has been up and the operating system, kernel and architecture it runs on, whether in a container, the time zone, the server clock's synchronization, the CPU cores and memory.
- **Dependencies** (admins): the database server's version and the migrations applied, and the FFmpeg version when the external encoder is set up.
- **Active configuration** (admins): per service, the settings that matter as read-only values (folders, intervals, addresses) and switches. A secret (database URL, error-reporting DSN, metrics password) only shows whether it is set.
- **Copy diagnostics:** puts what the page shows as plain text in the clipboard, for a bug report.

The engine reports itself over its gRPC link. The analyzer and the librarian write a heartbeat to the database every 30 seconds, which the API reads; it is not part of [backups](backups.md). Each new container of a worker is a new instance, so earlier ones stay listed as stopped: an admin takes them off with *Clear the stopped ones* (one that runs again comes back). See [API](api.md#routes) for `GET /about`.

### Breadcrumb

Above the title of every page that belongs to a section of the navigation, a breadcrumb says where it sits: the section (plain text, it has no page of its own), then the page, for instance `Library › Tracks` or `Station › Settings`. Pages outside a section (On air, Queue, History, Statistics, Account) have none.

A page inside a page adds its own name, last and not a link: `Library › Tracks › Space Cowboy` on a track, `Programming › Clocks › Morning drive` (or `New clock`) on a clock, `Library › Medleys › …` on a medley. The link back to the list brings it back as it was left: the same search, filters, sort and page (they are in its address, remembered while the app stays open).

The breadcrumb is built from the navigation's own list of pages, so a page added there gets one. It is a `nav` named *Breadcrumb*, its last item marked as the current page. Below 760 px, only the level above the page shows, as a back link (`‹ Tracks`), on pages inside a page.

## Development

```sh
cd web
npm ci
HERON_API_URL=http://localhost:8000 npm run dev   # against a running station's heron-web
npm test                                          # unit and component tests (Vitest), catalogs included
npm run typecheck && npm run build
```

Open a page with `?lang=pseudo` to check translations ([Pseudo-locale](#pseudo-locale)). `HERON_API_URL` defaults to `http://localhost:8080`, a `heron-api` run with `cargo run -p heron-api`. The API client is typed from the OpenAPI document ([API – Web client](api.md#web-client)). The app uses TanStack Query for requests and dnd-kit for dragging; routing is a small module on the History API (`src/lib/router.tsx`), since nginx answers every path with `index.html`.

Logic lives in `src/lib`, tested on its own. Components are tested rendered in jsdom with Testing Library: a test file starts with `// @vitest-environment jsdom`, and `src/test/render.tsx` renders with the app's providers, signed in, against API answers the test gives.

## Limitations

- The FX player and transport act at once; there is no undo for a skip.
- Only Icecast outputs and MP3 profiles have forms: the other kinds arrive with their encoders and sinks in [M11](roadmap/m11-sinks-live-dj.md) (the engine refuses them until then).
- An upload resumes after a dropped connection, but not after the page is reloaded. Covers in the buffer are loaded by the browser from the Cover Art Archive.
- The clock editor draws each slot with its category's average length: real tracks vary, so the hour as it sounds drifts from the drawing; simulate it to see real picks. A slot with an exact time is drawn at its time, and the slots after it from there; the simulation marks the exact slots and how they were met.
