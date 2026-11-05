# M14 – Several schedules

**Depends on:** M6, M13 (the schedule per station)

## Why

A station's week changes with the seasons: Christmas jingles and a Christmas clock in December, a summer grid in July and August, a special evening on New Year's Eve. With one schedule per station, someone has to repaint the week on the first day of the season and paint it back on the last, at the right hour, every year.

## Scope

Design: [Rotation – Scheduling](../rotation.md#scheduling).

- **Several schedules per station**, each a week of hours painted with clocks, as the schedule is today. One of them is the station's **default schedule**: an existing station's schedule becomes it, and a new station starts with an empty one.
- **Periods:** a schedule other than the default plays during its periods, from a date and hour to a date and hour (station time, the end excluded), optionally **every year** (the range may run over the new year). Outside its periods, the default schedule plays.
- **One schedule at a time:** when periods overlap, the one that started last plays (a New Year's Eve evening inside the Christmas period).
- **Hours a schedule leaves empty** rotate the whole library, as an hour without a clock does today: they do not fall back on the default schedule. A new schedule can start as a copy of another, so only the hours that change need painting.
- **Rotation log:** each pick names the schedule it came from.
- **Simulations** follow the periods.
- **API:** `GET`, `POST /schedules`; `GET`, `PUT`, `DELETE /schedules/{id}`; the active schedule and the next change. `GET`, `PUT /schedule` stay, for the default schedule.
- **Web UI:** on *Schedule*, a schedule picker, new (empty or a copy), rename, delete, the periods of a schedule, and which schedule plays now and the next change.

## Out of scope

- Making another schedule the default one.
- Periods shorter than an hour, or starting within an hour.
- A schedule shared by several stations.
- A webhook when the schedule changes.

## Deliverables

- Migration: `schedules`, `schedule_periods`, `schedule_id` on `clock_schedule` (instead of `station_id`); each station's schedule becomes its default schedule.
- `heron-rotation`: the schedule playing at a time, from the default schedule and the periods; the schedule in the explanation.
- `heron-api`: the `/schedules` routes, the default schedule for a new station, the schedules loaded with the station.
- `heron-web`: the schedule picker, periods, the active schedule and next change, translations.
- Docs: [Rotation](../rotation.md#scheduling), [Data model](../data-model.md#clockwheels), [API](../api.md), [Web UI](../web.md#schedules-and-periods).

## Acceptance criteria

- [x] An existing station upgrades with its schedule as its default schedule, and plays the same clocks.
- [x] A Christmas schedule with a period from 1 December 00:00 to 1 January 00:00, every year, plays its clocks from 1 December 00:00 to 31 December 23:59 each year, and the default schedule's before and after; a simulation across 1 December shows the change at midnight, station time.
- [x] A period inside another (31 December 20:00 to 1 January 02:00 inside Christmas) plays its own schedule, then Christmas comes back until the end of its period.
- [x] An hour the active schedule leaves empty rotates the whole library, even when the default schedule has a clock then.
- [x] Each pick in the rotation log names its schedule.
- [x] The default schedule cannot be deleted or given periods; a period that ends before it starts, or not on the hour, is refused.
- [x] *Schedule* shows which schedule plays now and the next change, and a new schedule copied from the default one has its hours.

## Questions

- **Overlapping periods.** Proposed: allowed, the one that started last plays. The other choice is to refuse them, which forbids a special evening inside a season.
- **Making another schedule the default one** (a new grid from September on). Proposed: later; a period without an end in sight does it in the meantime.

## References

- [Rotation](../rotation.md)
- [Several stations](../stations.md)
