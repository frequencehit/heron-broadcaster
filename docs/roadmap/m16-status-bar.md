# M16 – Status bar

**Depends on:** M5 (the web UI), M7b (the listener count), M13 (stations and roles)

## Why

While preparing the next hour in the Library or the Schedules, an operator cannot see what the decks are doing, how many people listen or whether the machine is struggling, without leaving the page. A line at the bottom of every page, as a terminal or an editor has, keeps the essentials in view. Nothing shows the load of the machine in the web UI today.

## Scope

- **One line at the bottom of every page**, one character high, in the monospace font, whatever the page. It stays in view while the page scrolls.
- **Both decks:** for each of A and B, the state (playing, paused, empty), whether it is the one on air, and the track with its elapsed time. An unreachable engine shows as such.
- **Listeners:** the total of the current station, from the same reading as the [Statistics](../statistics.md) page. A server that could not be read shows a gap, never zero.
- **System load** (operators and admins only): the CPU and memory in use (%) and the load average over 1, 5 and 15 minutes, of the machine the API runs on. Pushed on the live WebSocket every few seconds, as a new `system` message sent to operators and admins only.
- **Offline:** while the live connection is down, the bar says so and greys out.
- **Phones:** below 760 px the system part goes first, then the track titles; the bar stays on one line.

## Out of scope

- Disk space, temperature and per-process figures: the host is better watched with [its own exporters](../monitoring.md#what-else-could-be-monitored).
- History or graphs of the load: the bar shows the present only.
- Listeners of all stations added together.
- Choosing what the bar shows.

## Deliverables

- `heron-api`: a sampler task for the CPU, memory and load average, the `system` live message, sent to operators and admins only.
- `heron-web`: the status bar, translations, the layout making room for it.
- Docs: [Web UI](../web.md#status-bar), [API – Live WebSocket](../api.md#live-websocket), [Monitoring](../monitoring.md#what-else-could-be-monitored).

## Acceptance criteria

- [x] The bar is on every page, one line high at every width from a phone up, in light and dark themes, and never hides the end of a page.
- [x] Both decks follow the engine: loading, playing, pausing, skipping and emptying a deck change the bar within a second; the elapsed time moves smoothly.
- [x] The listener total equals the one the Statistics page shows, and a gap (`–`) when no server could be read.
- [x] An operator and an admin see the system load, moving with the machine's: a CPU burn makes the CPU rise within 10 s. A viewer receives no `system` message and sees no system part.
- [x] With the live connection down the bar says so; it recovers by itself.
- [x] Unit tests for the formatting, the `system` message and the roles; the component test covers viewer, operator, gap and offline.
- [x] Docs, translations (all catalogs) and screenshots are up to date.

## References

- [Web UI – Live updates](../web.md#live-updates)
- [API – Live WebSocket](../api.md#live-websocket)
- [Listener statistics](../statistics.md)
