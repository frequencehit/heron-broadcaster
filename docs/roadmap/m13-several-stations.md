# M13 – Several stations

**Depends on:** M6, M7, M7b, M8

## Why

A group running several stations (a main station and themed sister stations) wants them in one Heron: one library, one buffer zone, one analysis, the same artists and tags, and each station on air with its own programming. Running one Heron per station duplicates the library and its work, and cannot keep the stations apart: the same artist on two sister stations at the same minute.

## Scope

Design: [Several stations](../stations.md).

- **Stations:** name, code, time zone, engine address, enabled. An existing Heron becomes its first station, with everything it has.
- **Shared and per station:** the library, artists, categories, medleys, encoder profiles, users and some settings are shared; programming, restrictions, queue, playout, history, outputs, webhooks and the other settings are per station ([What is shared](../stations.md#what-is-shared-what-is-per-station)).
- **Categories tagged with stations:** one, several or all; badges with the stations' codes; a station only offers its categories; untagging refused while the station uses the category, listing what uses it.
- **Restrictions** apply to the stations they name, all by default.
- **Plays per station:** each play is a station's; tracks and artists show plays and last play per station and in total; library sort and filters by station.
- **Rules across stations:** artist, title and track windows against other stations' plays and planned queue, both ways, relaxed with the station's own rule of the same kind, explained in the selection log.
- **Medleys tagged with stations:** the rendered track goes into each station's medley category; songs from every category its stations can use unless the recipe narrows them; badges and a filter in the medley list.
- **Roles per station:** a shared role and a role on each station (`none`, `viewer`, `operator`, `admin`).
- **One engine per station**, each with its own outputs, state volume and handover; one queue manager per station in the API.
- **API:** per-station routes under `/stations/{id}/…`, public endpoints and webhooks per station, the single-station routes kept for the first station.
- **Web UI:** a station switcher, a Stations page, roles per station on the Users page, plays per station on track and artist pages.

## Out of scope

- A track's categories, weights, cue points and overrides differing between stations.
- Simulating several stations together.
- Moving categories or medleys between stations in bulk.
- Several Heron deployments sharing one library.

## Deliverables

- Migrations: `stations`, `category_stations`, `medley_stations`, `restriction_stations`, `user_station_roles`; `station_id` on clocks, schedule, picks, history, outputs, webhooks, listener totals and the per-station settings; existing rows to the first station.
- `heron-rotation`: rules across stations, their relaxation and explanation; the history of other stations as an input.
- `heron-api`: stations, per-station routes and roles, one queue manager and engine client per station, plays per station, public endpoints and webhooks per station, the single-station routes for the first station.
- `heron-librarian`: rendered medleys into each of their stations' medley categories.
- `heron-web`: station switcher, Stations page, category and medley badges and station tags, roles per station, plays per station, translations.
- Docs: [Several stations](../stations.md), [Data model](../data-model.md#stations), [Rotation](../rotation.md#rules-across-stations), [API](../api.md), [Web UI](../web.md), [Upgrading](../upgrading.md) (one engine service per station).

## Acceptance criteria

- [ ] An existing Heron upgrades to one station holding all its categories, clocks, schedule, restrictions, history, outputs, webhooks and settings, and plays on through the upgrade (hot handover) with nothing to set.
- [ ] Two stations, each with its own engine, play at once from the same library, each with its own clocks, schedule, queue, playout settings, EQ and outputs; a hot handover of one does not touch the other.
- [x] A category tagged with two stations is offered by both, one tagged with one is offered by that one only; untagging a station that uses the category is refused and lists what uses it.
- [x] Plays are recorded per station; a track and an artist show plays and last play per station and in total.
- [x] With an artist window across stations of 30 minutes, a week simulated on two stations never plays an artist on one within 30 minutes of the other (played or planned), unless the selection log shows that rule relaxed; the same for titles and tracks.
- [x] A medley tagged with two stations is in both stations' medley categories after its render; its songs come from the categories its stations can use, or from its recipe's.
- [x] A user with `none` on a station does not see it, in the API or the web UI; a `viewer` on one station and `operator` on another can run only the second.
- [x] The single-station API routes and public endpoints still answer for the first station.
- [x] A Heron with one station shows no station switcher, and works as before.

## Questions

- ~~**Liners of a medley on several stations.**~~ Decided: its first station's, one rendered file for all its stations.
- **Station-specific track details.** Should a track's weight in a category, or its overrides, ever differ between stations? Proposed: no, until a station asks.

## References

- [Several stations](../stations.md)
- [Rotation](../rotation.md)
- [Engine handover](../handover.md)
