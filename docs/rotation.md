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

Separation windows can be overridden per category (a power rotation category repeats its titles sooner than a gold category) and per clock slot. So can the [draw depth](#draw-depth).

Artist separation compares **artists, not names**: see [Artists with several names](#artists-with-several-names).

### Daypart separation

The windows count minutes since a play: they do not say where in the day it was, and a track heard in the morning show can come back in the afternoon. **Daypart separation** keeps a track out of the dayparts that follow the one it played in, so the listeners of each part of the day hear it at a different time.

- A station divides its day into **dayparts**, in its time zone, named A, B, C…: **4 dayparts of 6 hours** (the default) or **6 of 4 hours** (`daypart_count` in the `rotation` settings, 4 or 6). The division is the station's; the separation is each category's.

  | Division | A | B | C | D | E | F |
  |---|---|---|---|---|---|---|
  | 4 dayparts | 00:00–05:59 | 06:00–11:59 | 12:00–17:59 | 18:00–23:59 | | |
  | 6 dayparts | 00:00–03:59 | 04:00–07:59 | 08:00–11:59 | 12:00–15:59 | 16:00–19:59 | 20:00–23:59 |

- **Daypart separation N** (`dayparts` in a category's separation): a track of the category played in a daypart does not play in the N dayparts that follow it. 0 (or left out) turns it off, the default. With 4 dayparts, a track played in A:

  | Separation | Kept out of | Can play again in |
  |---|---|---|
  | 1 | B | C or D |
  | 2 | B, C | D |
  | 3 | B, C, D | the next day's A |

- **Same daypart, the following days X** (`daypart_days` in a category's separation, 0 to 7): a track of the category played in a daypart on one day does not play in that same daypart (the same letter) on the X days that follow. 0 (or left out) turns it off, the default. With X = 1, a track played in B on Monday:

  | Day | Its B |
  |---|---|
  | Monday | played |
  | Tuesday | kept out |
  | Wednesday | can play again |

  With X = 3, Tuesday's, Wednesday's and Thursday's B are kept out, and Friday's is free. Days are the station's, from midnight to midnight. X works **on its own**, with or without N: a category can keep its tracks out of the same hours of the next days while N is 0. Where N already keeps a daypart out, X changes nothing there. It looks at every play of the track in that daypart over the X days, not only its last play: a track played in B on Monday, then in D the same evening, is still kept out of Tuesday's B with X = 1.
- Dayparts follow one another **across midnight**: the next day's A follows D. N goes up to **6 with 4 dayparts and 10 with 6**, so the rule can run into the next days: with 4 dayparts and a separation of 6, a track played in A is kept out of B, C, D, then the next day's A, B and C, and can play again from the next day's D, at least 36 hours later.
- **The daypart it played in is not kept out**: whether the track plays again later in the same daypart is for the track separation to decide.
- It compares the **track itself**, not its title, artist or album, with its last play on the station (or its planned start, when it waits in the queue). Plays on other stations do not count: the [rules across stations](#rules-across-stations) are for that.
- Both values are set **per category**: a power rotation needs 1 or 2, a gold category 4 to 6 (and 1 or 2 days in the same daypart), and jingles and liners none, since they must be able to play at any hour. There is no station value. Clock slots do not override them. Categories are shared by the stations: a category's values count in each station's own dayparts, and N is cut to that station's maximum (6 with 4 dayparts).
- The values are the **slot's category's**: a track in several categories follows the one the slot called. Hours without a clock, which rotate the station's library, have no category value.
- **A track can be an exception:** its own N (`dayparts`, 0 to 10) and its own X (`daypart_days`, 0 to 7), each set or left unset on its own. A value the track sets **replaces** its category's, in **every category** it is in: a hit in a current category without separation can be spread over the day without moving it to another category, which would change its clocks and weights. 0 turns the rule off for that track; left unset, the category's applies, as before. The track's values are the same on **every station**, like its default [play ratio](#play-ratio): each station counts them in its own dayparts and cuts N to its maximum. They also apply in hours without a clock, where there is no category value, but not in slots with *enforce rules* off.
- **Clock changes:** times are the station's. When the clocks go back, the repeated hour stays in its daypart (02:00 is in A both times); when they go forward, the daypart holding the skipped hour is an hour shorter.
- It is a **separation rule**: it is relaxed like the others when a slot finds no candidate, halved (rounded down) then dropped, after album separation and before artist separation ([Rule relaxation](#rule-relaxation)). X is relaxed with N, at the same steps: halved (rounded down) at step 3, dropped at step 4. A track's own values are relaxed in the same way, at the same steps. It is not one of the dayparting [restrictions](#restrictions), which allow or ban a track in some hours and are never relaxed.
- The rotation log gives each pick's daypart and the daypart the track last played in, and counts the tracks each rule held back: `blocked.daypart` for N, `blocked.daypart_days` for X (a track both hold back counts for N), each track with its own values or its category's. For the track picked, it says which values applied: `daypart.separation_from` for N and `daypart.days_from` for X, `track` (its own) or `category`.

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

Restrictions are hard rules: unlike separation, **they are never relaxed**. With the [restrictions module](modules.md) turned off for a station, rotation applies none of them there; they are kept, and apply again once it is on. They apply to a track, an artist or a category (every track in it):

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
- A clock slot calling a category of that type right after one is **passed over** for the next slot, like a slot with nothing to play, and the rotation log says why (*same type twice in a row*). It is never relaxed: no relaxation step or fallback plays a jingle there instead. A slot calling the station's medley category is passed over the same way while [medleys are turned off](modules.md) for the station (*module turned off*).
- Hours without a clock never pick two of them in a row either.
- The clock editor warns about two slots in a row calling the same such type (the last slot followed by the first one included: the clock starts over), and simulations check the rule like the restrictions.

Types are compared one by one: a jingle right after an ad is allowed.

### Queue depth

Keep N tracks queued ahead: `queue.min_queued` ([API – Queue manager](api.md#queue-manager)). The default stays 3 rather than the 20 first proposed: tracks are picked for their planned start, and a deep queue plans further ahead, so dayparting and the clock's slots drift with every skip or manual insert.

### Clockwheel

An ordered list of slots. Each slot **calls a category**, and how it calls it matters as much as which category it is. Each slot has:

- a **category**;
- a **selection method**: least recently played, random or weighted (see [Draw depth](#draw-depth) for how each one draws);
- an **enforce rules** flag;
- separation overrides for this slot (see [Separation rules](#separation-rules)), and its draw depth (see [Draw depth](#draw-depth));
- a **linked to the next slot** flag, for a jingle that announces the next slot's track (see [Linked slots](#linked-slots));
- an optional **exact time**, hard or soft, for a slot that must start at a given minute and second of the hour (see [Exact times](#exact-times)).

Building good clocks has to be easy: the editor shows each hour as it will sound (slots, expected starts from the categories' average lengths, categories running short of tracks for their track separation), and a clock can be simulated before it goes on air. A category running short (a few jingles for a slot every quarter of an hour) only plays with its rules relaxed; the editor offers to turn *enforce rules* off on the slots calling it instead, which stops the separation rules there and keeps the restrictions.

How a clock runs:

- Each station hour starts its clock at the **first slot**; slots run in order. A clock shorter than the hour **starts over**; slots expected after the end of the hour do not play (the next hour starts its own clock). The track playing at the top of the hour finishes, unless the next hour's first slot has an [exact time](#exact-times).
- A clock can instead **loop across hours**, like SAM Broadcaster's clockwheel: it carries on from its last slot whatever the hour, so it can be longer than an hour (a 27-line wheel of two hours plays every line). Scheduled again after another clock, it resumes where it stopped.
- A slot that has nothing it may play, even relaxed, is skipped for the next one, and the pick says so.
- Hours without a clock rotate the **station's library**: least recently played, with the station's [draw depth](#draw-depth), rules enforced. The station's library is the tracks of the categories tagged with the station, except those also in a category it **keeps out** (`kept_out_category_ids` in the `rotation` settings: a category of tracks set aside, say). A track kept out does not play there even if another of its categories is the station's, and a track in none of the station's categories does not play there without a clock. A station with no category yet rotates the whole library, so a new Heron plays as soon as its music is imported.

Clockwheels are data stored in the database and edited in the UI. There is no scripting language (see [Locked decisions](decisions.md)).

#### Linked slots

A clock often announces a track with a jingle of its kind: *jingle 90s*, then a 90s track. A slot **linked to the next slot** plays only if the next slot's track plays right after it:

- Rotation picks the two together: the linked slot's track, then the next slot's track for when the first one ends. Both go to the rotation log; the first one says it is linked.
- **End of the hour:** when the next slot's track would start after the end of the hour, neither plays. Both slots are passed over, and the clock goes on with the slot after them, as long as it starts within the hour; the new hour starts with its own clock. A looping clock carries on across hours: the pair plays when the next hour runs the same clock.
- **Nothing found:** when the next slot finds nothing, even relaxed and with the fallback category (or is passed over as the same type twice in a row), the linked slot is passed over too.
- The rotation log says why: *its linked track would start after the hour*, or *its linked track found nothing*.
- The **last slot** can be linked: its next slot is the first one, when the clock starts over within the hour (or carries on, looping). At the end of the hour the new hour's clock does not count: the jingle is dropped.
- **No chains:** a linked slot cannot be followed by another linked slot (the clock is refused), so a clock of one slot cannot link it.
- The clock editor marks a linked slot as after the hour when its next slot's track is expected after the hour.
- A linked slot and its next slot start together at an [exact time](#exact-times) set on either of them.

In the queue, the jingle's entry is linked to its track's entry (`linked_to`). The engine keeps the link with its queue, across restarts and handovers:

- **Removing the track** removes its jingle, by hand or when the queue manager takes the track out. A jingle already playing plays on.
- **Removing the jingle** keeps the track: a track without its jingle misleads nobody.
- **Moving** the track, or the jingle, moves both: the jingle stays right before its track.
- **Nothing goes between them:** a track queued or moved there goes before the jingle.
- Skipping is unchanged. Tracks queued by hand are never linked.

#### Exact times

Without an exact time, the track playing at the top of the hour plays to its end, so the top of the hour, an ad break or a news flash drifts by seconds or minutes. A clock slot can have an **exact time**: minutes and seconds into the station hour (`:00:00` for the top of the hour, `:30:00` for a flash), and a mode.

- **Hard:** at the exact time, what plays fades out over 1.5 s and the slot's track starts, to the second. Tracks still waiting before it are dropped.
- **Soft:** nothing is cut while it can be avoided.
  - **Window:** the slot may start from *before* seconds before the exact time to *after* seconds after it (10 and 30 by default, 0 to 300 each, set per slot). Rotation plans the tracks before the slot so that the last one ends inside the window.
  - **Late tolerance:** a track ending inside the window plays to its end and the slot follows. A track that would end later is faded out at the end of the window, over 1.5 s, as in hard mode.
  - **Fill with a medley** (off by default): when the last track rotation could plan ends before the window, the gap is filled with one of the station's [medleys](medleys.md#shortened-plays), the one landing inside the window with the fewest parts left out, shortened from its end when too long. The usual rules apply to it (separation, restrictions, play ratio, never two in a row), and the medleys module must be on for the station.
  - **When nothing fits** (no medley, or the option off): a track of the station's **filler category** (`filler_category_id` in the `rotation` settings: short jingles, liners), else the slot starts early, the rotation log saying so.
- **Live** ([M11](roadmap/m11-sinks-live-dj.md)): an exact time will not cut a live DJ, unless the slot's *cut live* option is on (ad breaks). The option is stored with the slot; it does nothing until live DJs exist.

How rotation plans for it:

- **The upcoming exact time** is the first slot with one after the clock's position, in this run of the clock, or the next hour's first slot when the next hour's clock starts with one (an exact time at `:00:00` is usually the first slot of its clock). A slot linked to the next slot goes with it: when the slot before an exact one is [linked](#linked-slots) to it, the pair starts at the exact time; a linked slot can also have the exact time itself. A clock cannot set an exact time on both slots of a pair.
- **Before it**, each pick prefers the tracks that land: among those that pass the rules, the method chooses among the tracks ending inside the window if there are any, else among those ending before it, so more tracks can follow. In hard mode, the window is the exact time itself: what crosses it is cut, so a track that would cross it is kept only when no draw has one that does not, the draw widening to its maximum first. A pick far from the exact time is not concerned, since all its tracks end before the window: only the picks whose tracks can reach the window are.
- **Looking ahead:** one pick at a time can still end the hour with a gap to fill, when another order of the same slots would have ended in the window. So once the exact time is within **15 minutes** of a pick's planned start, or **4 slots or fewer** are left before it in this run of the clock, rotation simulates **10 endings** up to the exact slot before picking, each with its own seed (drawn from the pick's, so simulations replay the same). Each follows the real rules, pick by pick: the clock's slots, separation, draw depth, play ratio, relaxation, the preference for tracks that land and the gap fill; only the random draws differ. A simulated ending stops at the exact slot, or after 20 picks. The best ending is the one that, in this order:
  1. fades nothing out: the tracks before the exact slot end at the cut at the latest (the end of the window in soft mode, the exact time itself in hard mode). This comes first, whatever the rest: an ending that needs a filler jingle beats one that fades a song out;
  2. ends inside the window (at the exact time in hard mode), the closest to the exact time, a second late counting as 3 seconds early: ending before the exact time is better than after it, the track before then ending far from the cut and its fade (2 s late loses to 4 s early, but wins over 8 s early);
  3. fills the least time (medley or filler track);
  4. relaxes the fewest rules (the relaxation steps of its picks added up);
  5. plays the tracks played longest ago.

  Only the **first pick** of the best ending is queued: the next pick looks ahead again, from the queue as it then is (a track added or removed by hand included). The play ratio counters move only for the pick queued; within a simulated ending they stay as they are.
- **Slots that no longer fit** are passed over, as at the end of the hour: in soft mode, a slot whose tracks would all end after the window (a song slot with 40 s left) gives way to the next slot. In hard mode the slot plays, and its track is cut at the exact time.
- **When the exact time is due** (the planned start reaches the window, or the exact time in hard mode), the clock jumps to the exact slot: the slots between are passed over, and the slot starts when the track before it ends (inside the window), or at the end of the window (soft) or the exact time (hard), the track before being faded out.
- **When the exact slot comes before its window** (the slots before it ran out, or were passed over), the gap is filled: a medley (soft, with the fill on), else a track of the filler category landing in the window or ending before it, else the slot starts early. In hard mode a filler track may cross the exact time, and is cut there.
- **Missed exact times:** a slot whose exact time has passed in this run of the clock (a clock starting over, or a looping clock reaching it late) is passed over, never played late.
- The fades and cuts are the engine's: rotation gives the slot's queue entry its start time, and the engine fades out what plays and starts it then ([Audio engine – Exact times](audio-engine.md#exact-times)), on the server's clock. Listeners hear the stream a few seconds late (stream server buffers): the exact time is the engine's.
- **Tracks queued by hand** before an exact time are cut at it in hard mode. In soft mode, the queue manager takes them out of the queue when they no longer fit (they would end after the window); rotation's own tracks too, when they would start after it.
- **Clock changes:** exact times are in station time. When the clocks go back, the repeated hour runs its clock again, exact times included; when they go forward, the skipped hour's are skipped.

The rotation log says how each exact time was met (`exact` in the explanation of the slot's pick): the exact time (`at_unix_ms`), the mode and window, `met` (`on_time`, `early` or `late`) with the planned start's offset from the exact time (`offset_ms`, negative when early), whether the track before is faded (`faded`, and when: `cut_at_unix_ms`), and whether it is in its window. A pick filling a gap says so (`fill`: `medley` or `filler`, the exact slot it fills, for a medley its ID, how many hooks were left out and the parts left out by their position in the medley). Slots passed over say why: *the exact time is due*, *no longer fits before the exact time*, *exact time passed*. A pick made looking ahead says so (`lookahead`: the exact time, `draws`, and how the best ending meets it: `faded` when even the best one fades out the track before, `in_window`, `offset_ms`, where the tracks before the exact slot end from the exact time (negative when early), missing when no ending reached it in 20 picks, `filling_ms` and `relaxed_steps`).

**Planned starts** count the transitions: the next track starts at the track's **mix point**, where the engine starts it (the analysis' point, or the fixed crossfade, with the station's crossfade settings and the track's overrides, see [Audio engine](audio-engine.md)), else at its end. Rotation plans with each track's mix point into a full-length track; the queue manager and the queue's expected starts use the engine's own transition into the next waiting track (a jingle after a song plays back to back, for instance). A track that lands is one whose mix point falls in the window. Hard times are met by the engine whatever the plan.

**Simulations** run exact times without audio, the planned starts jumping to each exact time as the engine would; the audit checks each exact slot's start against its window (`exact_time` violation: *missed window*, an early start included). They look ahead as rotation does.

### Weights

Each track has a weight in each of its categories (1 by default), used by weighted slots. Plays (rotation or manual) multiply the track's weights by `weights.play_factor` (0.5 to 1), requests by `weights.request_factor` (1 to 2); weights stay between 0.1 and 10. Both factors are 1 by default: weights stay as set until a station chooses otherwise.

### Play ratio

A weight is a chance: it does not say how often a track plays. A **play ratio** is an exact quota instead: at 0.2, a track plays 2 times out of the 10 times rotation picks it, no more, no less.

- Each track has a ratio from **0.1 to 1**, in steps of 0.1 (stored in tenths, 1 to 10). It is **1 by default**, which changes nothing. A track has one ratio, the same in all its categories.
- That ratio is the track's **default**: a station can have **its own** for the track instead (0.3 on one station, the default on the others). A station's rotation, its log and its simulations use the station's ratio when it has one, the default otherwise.
- Each **station** keeps a **counter** per track, from 10 down to 1, then back to 10. A track never picked yet is at 10.
- When rotation picks a track that passes the restrictions and the separation rules (as relaxed at that step), its counter goes down one step, **whether the track plays or not**. The track plays if the counter was in the first `ratio × 10` steps: at 0.2 it plays at 10 and 9, and is held back from 8 to 1. At 1 it always plays.
- A track held back **leaves the slot to another candidate**: the slot's method picks again among the others. A track is offered at most once per pick: held back, it is not offered again for that pick, at a later relaxation step or in another slot. When every candidate of a step is held back, rotation goes on to the next relaxation step, as when a step has no candidate.
- The ratio is a **hard rule**, like the restrictions: it is never relaxed, at any step, the fallback category included.
- It applies in **every slot**, whatever its method, and in hours without a clock. It is **rotation only**: tracks queued by hand (and listener requests) always play and do not move the counter.
- The rotation log lists, for each pick, the tracks held back by their ratio with their counter (`held_by_ratio`: `track_id`, `ratio`, `counter`), and the ratio and counter of the track picked (`ratio`, `counter`): the station's ratio, as rotation used it.

**Weight or ratio?** They add up. The **weight** says which track rotation *offers*: a chance against the other tracks, per category, only in weighted slots (at 2, twice the chances of a track at 1). The **ratio** says whether the offered track *plays*: an exact quota out of 10, for the track itself, in every slot (on each station, the station's own ratio or the default). Rotation picks first (rules, then weights in a weighted slot), and the ratio decides after.

**A default ratio of 0 disables the track**: it is the same as *Disable*. A disabled track does not play at all: rotation, clocks and medleys never pick it, it cannot be queued by hand, and the queue manager takes it out of the queue. It stays in the library and in its categories, and keeps the ratio it had. **Re-enabling** a track, however it was disabled, asks for its ratio: the one it had, or 1. The counters can be **reset to 10** (all of the track's stations), from the track's page or when re-enabling it.

**A station's ratio of 0 disables the track on that station only.** That station's rotation, clocks and medleys never pick it, queuing it by hand there is refused, and that station's queue manager takes it out of its queue; it plays as usual on the other stations. A medley played on several stations leaves out the songs disabled on any of them. The station keeps the ratio the track had there: re-enabling it on the station asks for its ratio, the one it had there, or the default. A station's ratio can also go back to the default. *Disable* without a station stays global.

**Who disabled a track, and when**, is kept with it, for the global *Disable* and on each station; re-enabling forgets it.

A station's counter can be reset to 10 for one track, or **for all its tracks at once** (the rotation log's page, operators and admins of the station, after a confirmation).

**Importing ratios** from another system: a CSV file with one column per station (named by its code), an optional `default` column, and a column naming the track: `path`, its library path, or `filename`, its file name alone, ignoring case, as [Migrate](import.md) matches tracks. A ratio is written `0.2` or `0,2`, 0 disables the track (on that station, or everywhere in `default`), and an empty cell changes nothing. A dry run lists the rows that match no track (or several), the values that are not ratios and the unknown columns; nothing is written until it is applied.

The counter is state, not configuration: backups keep the ratios (the default and each station's) and who disabled a track, not the counters ([Backups](backups.md)). A simulation starts from the station's counters and moves copies of them in memory; the station's counters do not move.

### Draw depth

A slot in *least recently played* that always took the track played longest ago would play its category in the same order round after round, when nothing gets in the way: listeners hear the same sequence again and again. The **draw depth** breaks that, without a shuffled stack to keep:

- A slot only draws among the **X % of its category's tracks played longest ago** on the station, and picks among those that pass the rules: **at random** in a *least recently played* slot, **by weight** in a *weighted* slot.
- The depth is **20 % to start**, with **at least 3 tracks** (20 % of 5 jingles would leave one, so no draw at all; a category of 3 tracks or fewer draws among all of them). The percentage is of the tracks the restrictions allow at that time, rounded down.
- **Never-played tracks** are in the draw like the others: they count as the oldest, but are not picked before the others of the draw, so a category just imported does not play in import order. Tracks with the same last play, never played ones included, are ordered at random.
- When nothing in the draw passes the rules, the draw **widens by 10 points** (20 %, 30 %, 40 %, 50 %) **before any rule is relaxed** (see [Rule relaxation](#rule-relaxation)).
- It stops at its **maximum, 50 % by default**: the half of the category played most recently is never drawn from. The rules are then relaxed as before, with the draw kept at its maximum: a track a bit older with a looser rule is preferred to a track just played.
- The start and the maximum are set **per category** (`draw_start_pct` and `draw_max_pct` with its separation overrides, 1 to 100), and can be **overridden per clock slot**, like the separation windows. A category or a slot that sets neither follows the station's (`draw` in the `rotation` settings: `start_pct` 20 and `max_pct` 50 by default), which also applies to hours without a clock. A start above the maximum starts at the maximum.
- **Play ratio:** a track held back by its ratio leaves its place to another track of the same draw; the draw widens only when every track of it is held back or blocked by the rules.
- The order is **per station**: a category shared by several stations is ordered on each by that station's own plays, as the separation rules are.

How each method draws:

| Method | Draws among |
|---|---|
| Least recently played | the draw depth, at random |
| Weighted | the draw depth, by weight |
| Random | every track that passes the rules: no draw depth |

*Least recently played* in the strict sense (always the oldest track) is no longer offered: with the minimum of 3 tracks, it draws among the 3 oldest at least. Step 12 still takes the oldest track of the fallback category.

**Weight, ratio or depth?** The **depth** says *which tracks* rotation can draw from: the ones played longest ago, a share of the category. The **weight** gives each of them a chance against the others, in weighted slots only. The **ratio** decides whether the drawn track plays, an exact quota out of 10. Rotation takes the depth first, then the rules, then the weights, and the ratio decides last.

**Hours without a clock and the fallback category.** The station's library draws with the station's depth. At step 11 the fallback category draws with its own depth, widened from its start to its maximum as at step 0; without a fallback category, the station's library with the station's depth. Step 12 has no depth: it takes the least recently played track of the fallback category, without rules.

The rotation log gives, for each pick, the depth it was drawn at, the start and the maximum, how many tracks the draw had, and how many of them each rule held back at the pick's step (`draw`: `pct`, `start_pct`, `max_pct`, `tracks`, `blocked`): `draw: 30 %, 30 tracks, 4 held back by the artist`. A pick of a random slot, or of step 12, has none.

### Rule relaxation

When no track satisfies the rules, relax them step by step, then fall back to a safe category. Log every relaxation. **The queue must never be empty.**

The rule listeners notice least is relaxed first, and each rule is **halved before it is dropped**, so a small library gets "a bit less separation" rather than losing a rule outright.

| Step | Relaxation | With the default windows, and a category's daypart separation of 3 |
|---|---|---|
| 0 | All rules as configured, the [draw](#draw-depth) widened 10 points at a time from its start to its maximum | album 60, dayparts 3, artist 120, title 240, track 240 min; draw 20 %, 30 %, 40 %, 50 % |
| 1 | Album separation halved | album 30 |
| 2 | Album separation dropped | album off |
| 3 | Daypart separation halved, rounded down | dayparts 1 |
| 4 | Daypart separation dropped | dayparts off |
| 5 | Artist separation halved | artist 60 |
| 6 | Artist separation dropped | artist off |
| 7 | Title separation halved | title 120 |
| 8 | Title separation dropped | title off |
| 9 | Track separation halved | track 120 |
| 10 | Track separation quartered | track 60 |
| 11 | Fallback category, track separation still at a quarter, its draw widened from its start to its maximum | |
| 12 | Fallback category, least recently played track, no rules | |

- Rotation tries the steps in order, stops at the first one that yields at least one candidate, then applies the slot's selection method to those candidates. At step 0 it first widens the [draw](#draw-depth) to its maximum; steps 1 to 10 keep the draw at its maximum.
- Track separation is never dropped inside the slot's own category: playing the exact same recording again is the most audible repeat.
- Step 12 cannot fail while the fallback category has a track; picking the least recently played one still avoids an immediate repeat. If the fallback category is empty, the engine's fallback directory takes over.
- Slots with **enforce rules** off skip the separation rules entirely (step 0 without rules, then steps 11 and 12 only if restrictions leave nothing). Restrictions still apply.
- Windows are those of the station (`rotation` settings), overridden by the slot's category, then by the slot; the daypart separation is the category's, or the track's own ([Daypart separation](#daypart-separation)); steps 1 to 10 relax those. Steps 11 and 12 use the fallback category's windows; without a fallback category set, the station's library is the fallback.
- Every relaxed pick records its step in `play_history.relaxation_applied` and in the rotation log.

**Why artist before title** (the handoff proposed title first): repeating an artist within two hours is common and accepted on radio, while hearing the same song again, even in another version, is a clear repeat to listeners. **Dayparts before the artist:** listeners notice a track coming back in the next daypart less than an artist repeated within the hour.

The steps were numbered 0 to 10 before daypart separation; a migration renumbered the steps of the picks already logged (`rotation_picks` and `play_history.relaxation_applied`), so the log reads the same.

### Rules across stations

With [several stations](stations.md#rules-across-stations), a station can also keep the artists, titles and tracks it plays apart from what other stations played or will play, each within its own window (`across` in the `rotation` settings: `artist_min`, `title_min`, `track_min`, 0 for off, and `station_ids`, the stations it looks at, all the others when empty). The window looks both ways: plays of the other stations before the pick, and their queued tracks at their planned times, before or after it.

They are relaxed with the station's own rule of the same kind: artist halved at step 5 and dropped at step 6, title at 7 and 8, track at 9 and 10; they are off in the fallback category (steps 11 and 12) and in slots with enforce rules off. The rotation log counts the tracks they held back (`blocked.across`) and names, for the least recently played of them, the station and the play or planned track that held it back (`held_elsewhere`). A simulation takes the other stations' history and queues as they are, and its audit checks the picks against them too (`across_stations` violations).

### Scheduling

Clocks are assigned per hour and day of week (`clock_schedule` in the [data model](data-model.md)), in the **station's time zone** (`station` settings, an IANA name such as `Europe/Paris`; the server's `TZ`, or UTC, by default). When the clocks go back, the repeated hour runs its clock again; when they go forward, the skipped hour is skipped.

A station has **several schedules** ([M14](roadmap/m14-schedules.md)), each a week of hours painted with clocks, and **one plays at a time**:

- The **default schedule** plays unless another one's period says otherwise. It cannot be deleted and has no periods. An existing station's schedule became its default schedule; a new station starts with an empty one.
- Another schedule plays during its **periods**: from a date and hour to a date and hour, in station time, the end excluded (1 December 00:00 to 1 January 00:00 is the whole of December). Periods start and end on the hour. A period can come back **every year**: the years are ignored, and the range may run over the new year.
- When periods overlap, the one that **started last** plays: a New Year's Eve evening from 31 December 20:00 inside the Christmas period plays its own schedule, then Christmas comes back until its end.
- An hour the playing schedule leaves empty rotates the station's library, as an hour without a clock: it does **not** fall back on the default schedule. A new schedule can start as a copy of another, so that only the hours that change need painting.
- Each pick in the rotation log names the schedule it came from (`schedule_id`, `schedule_name` in its explanation), and simulations follow the periods.
- A track picked ahead for the next hour is picked from the schedule playing at its planned start: the change happens at the hour, like a change of clock.

## Rotation model

- Rotation is a **pure function** of (library, play history, rules, current clock, current time), in the `heron-rotation` crate. The library carries the station's ratio counters; the caller moves them after each pick (`Library::count_down`) and saves them.
- The history is what played on the decks (on the FX player, only the jingles rotation picked to play over an intro) as far back as the longest window looks, followed by the queue at its planned times: the queue manager picks each new track for when it should start.
- It returns the next track plus an **explanation**: which clock and slot, the windows applied (with the picked track's daypart values), the relaxation step, how many tracks each rule held back or restrictions forbade, how many were left to choose from, and the dayparts (`daypart`: the station's `count`, the pick's `daypart` from 0 for A, the daypart `separation` and the `days` before relaxation, where they came from, `separation_from` and `days_from`, `track` or `category`, and `last_daypart`, where the track last played), and the draw depth (`draw`, see [Draw depth](#draw-depth)). Each pick is kept in the **rotation log** (`rotation_picks`); its play is linked to it and carries its step (`play_history.relaxation_applied`).
- This makes it possible to **simulate** days of programming offline and test it without audio. A simulation is **audited** without trusting the log: each pick's windows are recomputed from its slot, its track's own daypart values and its step, and the pick is compared with every play before it, one by one, daypart separation included (a `daypart` or `daypart_days` violation names the earlier play); restrictions are checked again, and so is the draw depth: the tracks of the pick's pool that the restrictions allowed and that played longer ago than it (or never) are counted, and a pick with as many as its draw had, or more, is a `draw` violation (the draw is the one logged at step 0 and step 11, within the start and the maximum, and the maximum at steps 1 to 10); and so is the play ratio: the counters are replayed from the station's, and a pick whose counter was in its held-back steps is a violation (tracks held back are not picks, so they are never flagged). Simulations run from the API (`POST /rotation/simulate`) and the web UI, and in the tests on a synthetic station over a week that crosses a daylight saving change.
