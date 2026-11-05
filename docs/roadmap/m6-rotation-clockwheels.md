# M6 – Rotation & clockwheels

**Depends on:** M4

## Scope

Clock construction and the way slots call their categories are the heart of a good programme: this milestone gets as much care as the audio engine.

- Categories, separation rules (title, artist, album, track), queue depth.
- Separation windows overridable per category and per clock slot.
- Clocks as ordered slots; how each slot calls its category: selection method, enforce-rules flag, separation overrides; weights on play / request.
- **Artists with several names:** artists, their names and similar artists; tracks linked to one or more artists (split on "featuring"); artist separation by artist, not by name. MusicBrainz relationships offered as suggestions once [M1b](m1b-library-management.md) identifies tracks.
- **Restrictions**, never relaxed: dayparting (days and hours), date ranges, and sequences (a category never directly after another), on tracks, artists and categories.
- Rule relaxation and fallback category, with logging.
- Clock scheduling by hour and day of week.
- Drag-and-drop clock editor in the UI, showing each hour as it will sound; editors for artist names and restrictions.
- Rotation log explaining why each track was picked.

## Acceptance criteria

- [x] Simulation mode generates 7 days of programming and reports rule violations: zero, except logged relaxations.
- [x] The simulation never breaks a restriction, relaxed steps included.
- [x] Two names of the same artist (or of an artist group counted as one) are never played closer than the artist separation, except in logged relaxations.

Demonstrated by `crates/rotation/tests/simulation.rs` (a synthetic station of about 2,900 tracks, a day and a night clock with an unscheduled afternoon, five restrictions, similar artists, a week crossing a daylight saving change, several seeds; the audit is checked against broken picks in `sim.rs`), and end to end by `e2e::rotation_follows_clocks_restrictions_and_logs_its_picks` (`POST /rotation/simulate`). Artist groups became similar artists (see [Decisions](../decisions.md#decisions-made-after-the-handoff)).

## References

- [Rotation](../rotation.md)
- [Data model – Clockwheels](../data-model.md#clockwheels)
- [Rotation – Rule relaxation](../rotation.md#rule-relaxation)
- [Rotation – Artists with several names](../rotation.md#artists-with-several-names), [Restrictions](../rotation.md#restrictions)
