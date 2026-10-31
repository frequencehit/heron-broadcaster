# Rotation

Modeled on SAM Broadcaster's "Playlist Rotation Rules" and "Clockwheel (category rotation)". Delivered in [M6 – Rotation & clockwheels](roadmap/m6-rotation-clockwheels.md); the code lives in the `heron-rotation` crate.

What makes a programme sound good is mostly invisible to listeners: how the clocks are built, how each slot calls its category, the restrictions, and the time between two plays of a title or an artist. This is the heart of the product, and it deserves as much care as the audio.

## Rules

### Separation rules

Configurable, in minutes. Do not play the same:

| Rule | Default |
|---|---|
| Album within | 60 min |
| Artist within | 120 min |
| Title within | 240 min |
| Track within | 240 min |

Separation windows can be overridden per category (a power rotation category repeats its titles sooner than a gold category) and per clock slot.

Artist separation compares **artists, not names**: see [Artists with several names](#artists-with-several-names).

### Artists with several names

The same artist often appears under several names: solo name, band, side projects, spellings (`Neal Morse`, `The Neal Morse Band`). Comparing tag strings would play them back to back. So:

- An **artist** has one or more **names**. Tracks are linked to artists through their tags: a name links the track to its artist. Names are matched case-insensitively and ignoring accents and extra spaces (`NÉAL  MORSE` is `Neal Morse`); a name never seen before becomes a new artist.
- The artist tag is split into several artists **on "featuring" only** (`feat.`, `ft.`, `featuring`), and artists featured in the title count too (`Get Lucky (feat. Pharrell Williams)`). `&`, `and`, commas and slashes never split a name: `Simon & Garfunkel` and `Earth, Wind & Fire` are one artist each.
- A track can have several artists (featuring, duets, collaborations); artist separation applies to each of them.
- Artists can be marked **similar**: separation treats them as the same artist (a band and its members' solo projects, a duo and its singers). The link is between two artists and is not transitive: if Genesis is similar to Phil Collins and to Peter Gabriel, Collins and Gabriel can still follow each other. Similar links replace the artist groups sketched before M6.
- Anything less obvious is done by hand in the UI, never guessed: a name moved to another artist (or split off as an artist of its own), two artists merged, two artists marked similar.
- **Suggestions** wait for review and are never applied alone: a name made of known artists' names (`Simon & Garfunkel` when `Simon` is an artist), and MusicBrainz relationships between artists of the station ("member of band", "is person", "subgroup", "collaboration"), looked up on request by `heron-librarian` for the artists whose MusicBrainz ID the tags give ([M1b](roadmap/m1b-library-management.md) writes them).
- Artists nothing refers to any more (no track, no hand-made link, restriction or MusicBrainz ID) are forgotten.

### Restrictions

Restrictions are hard rules: unlike separation, **they are never relaxed**. They apply to a track, an artist or a category (every track in it):

- **dayparting:** allowed or banned on given days and hours (no explicit lyrics before 22:00, no long tracks during the morning show). Hours that run past midnight belong to the day they start on: Friday 22:00 to 02:00 covers Saturday 01:30;
- **date ranges:** seasonal content only in its season (optionally every year, the range running over the new year if needed), or a track banned for a while;
- **sequence:** never directly after a track of a category (no two ballads in a row: the Ballads category, never after Ballads).

A **ban** forbids the track while it matches. **Allow** rules restrict the track to where they match: for each target and kind of rule, at least one must match (two dayparts allowed on a category add up; an allowed daypart on the category and one on the artist must both hold). Times are the track's planned start, in the station's time zone.

When a slot finds no candidate that respects the restrictions, it goes through the relaxation steps of the separation rules, and then to the fallback category, still respecting the restrictions. If even the fallback has nothing allowed, the slot is skipped for the next one in the clock; if no slot can play, nothing is queued and the engine's fallback directory takes over.

### Total time

*Categories* shows, next to each category's number of tracks, their **total time** (`hh:mm:ss.ss`, days in front when needed): how long the category plays before its titles come back, to check it can feed the slots calling it. Each track counts for what is played of it (cue-in to cue-out, overrides applied), or its file's length when not analyzed yet; every track of the category that is not in the trash counts, disabled and missing ones included, as in the number of tracks. The API gives it as `total_ms`. The columns sort by a click on their header.

### Category types

Every category has a **type**: what it holds. **Song**, **Jingle** and **Ad** exist from the start; a station renames them, adds its own (Liner, Promo, Voice track…) and deletes those no category uses (*Categories*, at the bottom). A category can also have no type.

Types let rules apply to every category of a kind at once, whatever their number: a station with nine jingle categories sets a rule once, on the Jingle type. Categories that existed before types were added got Jingle when the FX player offered them, Song otherwise: a guess to check. The [import from another software](import.md) gives Jingle to the categories it marks as jingles.

### Never two in a row

A category type can be set to **never play twice in a row** (*Categories › Category types*); Jingle is from the start, any other type can be (Ad, Liner…). Then:

- Rotation **never picks a track of that type right after a track of the same type**: what played or is queued just before, whatever its category (nine jingle categories count as one), a track queued by hand included. FX player tracks do not count: they play over the music.
- A clock slot calling a category of that type right after one is **passed over** for the next slot, like a slot with nothing to play, and the selection log says why (*same type twice in a row*). It is never relaxed: no relaxation step or fallback plays a jingle there instead.
- Hours without a clock never pick two of them in a row either.
- The clock editor warns about two slots in a row calling the same such type (the last slot followed by the first one included: the clock starts over), and simulations check the rule like the restrictions.

Types are compared one by one: a jingle right after an ad is allowed.

### Queue depth

Keep N tracks queued ahead: `queue.min_queued` ([API – Queue manager](api.md#queue-manager)). The default stays 3 rather than the 20 first proposed: tracks are picked for their planned start, and a deep queue plans further ahead, so dayparting and the clock's slots drift with every skip or manual insert.

### Clockwheel

An ordered list of slots. Each slot **calls a category**, and how it calls it matters as much as which category it is. Each slot has:

- a **category**;
- a **selection method**: least recently played, random or weighted;
- an **enforce rules** flag;
- separation overrides for this slot (see [Separation rules](#separation-rules)).

Building good clocks has to be easy: the editor shows each hour as it will sound (slots, expected starts from the categories' average lengths, categories running short of tracks for their track separation), and a clock can be simulated before it goes on air. A category running short (a few jingles for a slot every quarter of an hour) only plays with its rules relaxed; the editor offers to turn *enforce rules* off on the slots calling it instead, which stops the separation rules there and keeps the restrictions.

How a clock runs:

- Each station hour starts its clock at the **first slot**; slots run in order. A clock shorter than the hour **starts over**; slots expected after the end of the hour do not play (the next hour starts its own clock). The track playing at the top of the hour finishes: there is no hard sync yet.
- A clock can instead **loop across hours**, like SAM Broadcaster's clockwheel: it carries on from its last slot whatever the hour, so it can be longer than an hour (a 27-line wheel of two hours plays every line). Scheduled again after another clock, it resumes where it stopped.
- A slot that has nothing it may play, even relaxed, is skipped for the next one, and the pick says so.
- Hours without a clock rotate the **station's library**: least recently played, rules enforced. The station's library is the tracks of the categories tagged with the station, except those also in a category it **keeps out** (`kept_out_category_ids` in the `rotation` settings: a category of tracks set aside, say). A track kept out does not play there even if another of its categories is the station's, and a track in none of the station's categories does not play there without a clock. A station with no category yet rotates the whole library, so a new Heron plays as soon as its music is imported.

Clockwheels are data stored in the database and edited in the UI. There is no scripting language (see [Locked decisions](decisions.md)).

### Weights

Each track has a weight in each of its categories (1 by default), used by weighted slots. Plays (rotation or manual) multiply the track's weights by `weights.play_factor` (0.5 to 1), requests by `weights.request_factor` (1 to 2); weights stay between 0.1 and 10. Both factors are 1 by default: weights stay as set until a station chooses otherwise.

### Rule relaxation

When no track satisfies the rules, relax them step by step, then fall back to a safe category. Log every relaxation. **The queue must never be empty.**

The rule listeners notice least is relaxed first, and each rule is **halved before it is dropped**, so a small library gets "a bit less separation" rather than losing a rule outright.

| Step | Relaxation | With the default windows |
|---|---|---|
| 0 | All rules as configured | album 60, artist 120, title 240, track 240 min |
| 1 | Album separation halved | album 30 |
| 2 | Album separation dropped | album off |
| 3 | Artist separation halved | artist 60 |
| 4 | Artist separation dropped | artist off |
| 5 | Title separation halved | title 120 |
| 6 | Title separation dropped | title off |
| 7 | Track separation halved | track 120 |
| 8 | Track separation quartered | track 60 |
| 9 | Fallback category, track separation still at a quarter | |
| 10 | Fallback category, least recently played track, no rules | |

- Rotation tries the steps in order, stops at the first one that yields at least one candidate, then applies the slot's selection method to those candidates.
- Track separation is never dropped inside the slot's own category: playing the exact same recording again is the most audible repeat.
- Step 10 cannot fail while the fallback category has a track; picking the least recently played one still avoids an immediate repeat. If the fallback category is empty, the engine's fallback directory takes over.
- Slots with **enforce rules** off skip the separation rules entirely (step 0 without rules, then steps 9 and 10 only if restrictions leave nothing). Restrictions still apply.
- Windows are those of the station (`rotation` settings), overridden by the slot's category, then by the slot; steps 1 to 8 relax those. Steps 9 and 10 use the fallback category's windows; without a fallback category set, the station's library is the fallback.
- Every relaxed pick records its step in `play_history.relaxation_applied` and in the selection log.

**Why artist before title** (the handoff proposed title first): repeating an artist within two hours is common and accepted on radio, while hearing the same song again, even in another version, is a clear repeat to listeners.

### Rules across stations

With [several stations](stations.md#rules-across-stations), a station can also keep the artists, titles and tracks it plays apart from what other stations played or will play, each within its own window (`across` in the `rotation` settings: `artist_min`, `title_min`, `track_min`, 0 for off, and `station_ids`, the stations it looks at, all the others when empty). The window looks both ways: plays of the other stations before the pick, and their queued tracks at their planned times, before or after it.

They are relaxed with the station's own rule of the same kind: artist halved at step 3 and dropped at step 4, title at 5 and 6, track at 7 and 8; they are off in the fallback category (steps 9 and 10) and in slots with enforce rules off. The selection log counts the tracks they held back (`blocked.across`) and names, for the least recently played of them, the station and the play or planned track that held it back (`held_elsewhere`). A simulation takes the other stations' history and queues as they are, and its audit checks the picks against them too (`across_stations` violations).

### Scheduling

Clocks are assigned per hour and day of week (`clock_schedule` in the [data model](data-model.md)), in the **station's time zone** (`station` settings, an IANA name such as `Europe/Paris`; the server's `TZ`, or UTC, by default). When the clocks go back, the repeated hour runs its clock again; when they go forward, the skipped hour is skipped.

A station has **several schedules** ([M14](roadmap/m14-schedules.md)), each a week of hours painted with clocks, and **one plays at a time**:

- The **default schedule** plays unless another one's period says otherwise. It cannot be deleted and has no periods. An existing station's schedule became its default schedule; a new station starts with an empty one.
- Another schedule plays during its **periods**: from a date and hour to a date and hour, in station time, the end excluded (1 December 00:00 to 1 January 00:00 is the whole of December). Periods start and end on the hour. A period can come back **every year**: the years are ignored, and the range may run over the new year.
- When periods overlap, the one that **started last** plays: a New Year's Eve evening from 31 December 20:00 inside the Christmas period plays its own schedule, then Christmas comes back until its end.
- An hour the playing schedule leaves empty rotates the station's library, as an hour without a clock: it does **not** fall back on the default schedule. A new schedule can start as a copy of another, so that only the hours that change need painting.
- Each pick in the selection log names the schedule it came from (`schedule_id`, `schedule_name` in its explanation), and simulations follow the periods.
- A track picked ahead for the next hour is picked from the schedule playing at its planned start: the change happens at the hour, like a change of clock.

## Rotation model

- Rotation is a **pure function** of (library, play history, rules, current clock, current time), in the `heron-rotation` crate.
- The history is what played on the decks (on the FX player, only the jingles rotation picked to play over an intro) as far back as the longest window looks, followed by the queue at its planned times: the queue manager picks each new track for when it should start.
- It returns the next track plus an **explanation**: which clock and slot, the windows applied, the relaxation step, how many tracks each rule held back or restrictions forbade, how many were left to choose from. Each pick is kept in the **selection log** (`rotation_picks`); its play is linked to it and carries its step (`play_history.relaxation_applied`).
- This makes it possible to **simulate** days of programming offline and test it without audio. A simulation is **audited** without trusting the log: each pick's windows are recomputed from its slot and step, and the pick is compared with every play before it, one by one; restrictions are checked again. Simulations run from the API (`POST /rotation/simulate`) and the web UI, and in the tests on a synthetic station over a week that crosses a daylight saving change.
