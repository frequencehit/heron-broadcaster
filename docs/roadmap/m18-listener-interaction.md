# M18 – Listener interaction

**Depends on:** M7 (webhooks and public endpoints), M13 (several stations), modules (#148)

## Why

Listeners want to take part: ask for a song with a dedication, choose the next song, say whether they like the track playing. Heron gives station websites read-only public endpoints; it has no way for a listener to send anything, and a crowd of listeners must never reach the API that runs the stations. This milestone is the common base of the **show modules**: Dedications (#131), Match (#132), Likes (#133) and Top 30 (#134) are built on it. See [Listener interaction](../listener-interaction.md).

## Scope

- **`heron-public`, a public gateway** of its own (a binary and a container): the only part of Heron listeners reach, without database access, talking to the API with a gateway token that only the gateway routes accept.
  - Reads from its cache: one request a second to the API for every station's state (now playing, the shows), fanned out to every widget over Server-Sent Events; the API's public endpoints cached.
  - Writes checked before they go on: limits per address, the show on and open, a proof of work computed by the browser, a limit per device, field sizes, a limit on what reaches the API per second. Sent on one by one, or added up and sent every second in batches.
  - CORS: reads from any origin, writes from the station's websites only.
  - Its own metrics (requests, refusals, open event streams) on a port kept off the public one.
- **Show modules** registered as [modules](../modules.md), off by default: Dedications, Match, Top 30.
- **Per show and per station**: opening hours in whole hours, in the station's time zone; an hour belongs to one show at most (an overlap is refused when saved); outside its hours, a message with the next opening, or nothing.
- **The website**: per show, a dedicated page and an optional small widget, one line each, with the show's color and title; a page of its own on the gateway.
- **Notifications** through the station's webhooks: `show.upcoming`, `show.started` and `show.ended`, per show: which webhooks, each event on or off with its message and placeholders, the language of its dates.
- **The web interface**: a *Shows* section in the menu, one page per show with its hours (and the other shows' hours), its notifications with a preview, and its website settings with the lines to copy and a live preview.
- **Personal data**: no IP address stored; the device's hash kept in memory until the range ends; what is kept and for how long, written in the docs.

## Out of scope

- What each show does while it is open (the dedication form, the vote, likes, the chart): #131, #132, #133, #134, each on top of this base.
- Turnstile and hCaptcha: the proof of work needs no outside service.
- The gateway on the About page.

## Deliverables

- `heron-public`: the binary (`crates/public`), its image (`docker/rust.Dockerfile`), its service in `docker-compose.yml`.
- `heron-core`: the show modules, opening hours and their checks.
- `heron-api`: `GET`, `PUT /stations/{id}/shows/{module}`, `GET /stations/{id}/shows`; `GET /gateway/state` and the gateway's token check; the show events in webhooks; the station's `gateway_url`.
- `heron-web`: the *Shows* section and its pages, translations (all catalogs).
- Docs: [Listener interaction](../listener-interaction.md), [API](../api.md#the-public-gateway), [Web UI](../web.md#shows), [Modules](../modules.md), [Architecture](../architecture.md), [Monitoring](../monitoring.md#heron-public), [Upgrading](../upgrading.md#edge).

## Acceptance criteria

- [ ] `heron-public` builds into its own image for amd64 and arm64 and starts with `docker compose up`; without the gateway token it answers `503` and the API refuses it.
- [ ] A widget follows a station over Server-Sent Events; any number of them cost the API one request a second.
- [ ] The gateway token reaches the gateway routes only; user sessions and API tokens do not reach them.
- [ ] Writes over a limit (per address, per device, per second to the API) get `429`; a write without a valid proof gets `403`; a field too long gets `422`.
- [ ] Opening hours are saved per show in whole hours; hours another show of the station has are refused, saying which.
- [ ] Outside its hours a show refuses writes, and its widget shows the message with the next opening, or nothing.
- [ ] A show turned off answers `404` on its routes, in the gateway too, and sends nothing.
- [ ] `show.upcoming`, `show.started` and `show.ended` reach the chosen webhooks with their message.
- [ ] Tests: the gateway's limits, proof and cache; hours and their overlaps; the API's routes end to end; the web pages. Docs and translations are up to date.

## References

- [Listener interaction](../listener-interaction.md)
- [Modules](../modules.md)
- [API – Webhooks](../api.md#webhooks)

## Match every track, from the clock

Asked after the first evenings on air: a match on every track of the hours, and the two tracks taken from the hour's clock rather than one category, so that the matches keep the clock's balance, jingles and exact times ([Match](../listener-interaction.md#match)).

- [ ] `every_tracks` accepts 1: each winner carries the next vote; the next match is queued behind the winner at the close, and its vote opens when the winner starts; a match that cannot run lets its slot's track carry the next vote.
- [ ] `tracks_from: clock` picks both tracks as the next music slot of the hour's clock would, twice; the winner counts as that slot's pick; jingle and exact-time slots play between the vote's track and the winner, the vote closing 10 seconds before the first of them; an hour without a clock takes the match's category.
- [ ] `announce_every` plays the announcement every X matches.
- [ ] The first match of a range votes during the last track before it, its winner the first track of the range (after an exact time at its start); the last match is the last whose winner starts before the range closes; the show counts as open from the first vote.
- [ ] The queue lock covers what the clock put between the vote's track and the winner.
- [ ] The settings form: *the tracks of the match* (the clock or a category), every track, the announcement every X matches; the On air panel says *every track*.
- [ ] Unit tests for the planner (every track, the clock's slots, a jingle and an exact time between, a slot without two tracks, an hour without a clock, a range starting at an exact time and one starting mid-hour); the match e2e test runs a chain of three matches from a clock.

