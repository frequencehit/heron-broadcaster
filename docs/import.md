# Importing from another software

A station moving to Heron from another radio automation software keeps years of work in it: titles and artists fixed by hand, categories, fade lengths, intro and hook points. **Migrate** (admins, under *Library*) brings them into the library from an export of the other software, without touching the audio files. Every import can be undone.

## The export

A **JSON Lines** file: one JSON object per line, one line per audio file. A JSON array of the same objects works too. Unknown fields are ignored, so an export can carry more than Heron reads.

| Field | Meaning |
|---|---|
| `filename` | The audio file's name. A folder in front of it is ignored: rows are matched to library tracks by file name alone, ignoring case. |
| `title`, `artist`, `year` | Tags. Backslashes stored before quotes (`What\'s Up`) are removed. |
| `created_at` | When the track was added (`2022-03-11 09:35:38`): becomes its date added in Heron. |
| `category_id`, `category_name`, `category_parent_id`, `category_can_schedule`, `category_is_jingle` | The track's category in the other software. |
| `problem_id`, `problem_name` | A problem the other software marked the track with; 0 for none. |
| `final_duration` | The file's length, in seconds, to check it is the same edit. |
| `cue_in` | Where the other software cut the file, in seconds. The other times are on the uncut file: the import subtracts `cue_in` from them. |
| `intro`, `intro2` | Where the intro (a second intro) ends: the vocals start. |
| `hook_in`, `hook_out` | The hook: the most recognizable part of the song. |
| `start_next_soft`, `start_next_hard` | Where the next track starts after this one, with a fade or hard. |
| `fade_in`, `fade_out` | Fade lengths, in seconds. |

Times are seconds; **0 means "not set"**. A point that falls at 0 or before, or after the end of the file, once moved by `cue_in` is left out, and the dry run says so.

`cue_in` and `cue_out` themselves are not imported: they are where the other software cut the file, and Heron finds its own cue points on the file it plays.

For the station this was written for, the export comes from its MariaDB database with a short PHP script (`json_encode` of each row, one per line).

## What it sets

| In the export | In Heron |
|---|---|
| title, artist, year | Tags kept in Heron's database (`track_tag_overrides`, see [Library management](library.md#keeping-tags-in-heron-instead-of-the-files)): the files are left alone, whatever `write_tags` says |
| category | The Heron category named in the dry run, created if missing with the type chosen there (Jingle for the export's jingle categories, Song otherwise); its tracks can be disabled, and a created category can be offered by the FX player |
| problem | Nothing, a label, or a label and the track disabled, as chosen in the dry run |
| fade-in, fade-out | The track's crossfade overrides: the station's fade, with the export's length |
| intro, second intro, hook, next track | The track's markers (`track_markers`), shown on its page |
| created_at | The track's date added |

The markers are shown on the track's waveform and in its legend. The intro point and the hook can also be set or fixed by hand on the track's page; undoing an import puts back the points the track had before it. The intro point places the jingles played over a song's intro ([Audio engine](audio-engine.md#jingles-over-the-intro)); the other markers are kept but not played by yet: transitions are still planned by Heron's own rules (tracks shorter than `min_track_ms`, jingles for instance, are never crossfaded).

## Dry run, conflicts and apply

1. **Choose the file.** The browser reads it and sends its rows to `POST /api/v1/library/import/preview`, which writes nothing.
2. **Categories**: each category of the export, its number of files, the name of the Heron category its tracks go in (created if missing; empty for none), whether the FX player offers it, and whether its tracks are disabled. Categories the other software never schedules start disabled, jingle categories start offered by the FX player.
3. **Problems**: each problem, and what it becomes (nothing, a label, a label and the track disabled), with the label's name.
4. **Files**, filtered by: found, conflicts, other length, values left out, not imported (not in the library, twice in the export, several library tracks with that name). Files whose length differs from the library's by more than 2 s are probably another edit: they are left out until ticked.
5. **Conflicts**: a field Heron already has **set in Heron** (a tag kept in Heron, a fade override, a marker) with another value than the export's. Values Heron only read from the file are replaced without asking. For each track with conflicts: **Edit** (both values, and a field to type the one to keep), **Use the import**, or **Keep Heron's** (its other fields are still imported); or one of the last two for every conflict at once. The import cannot be applied until every conflict is settled.
6. **Import**: `POST /api/v1/library/import/apply` applies everything in one transaction. For 3,400 tracks, it takes a few seconds.

## Undo

*Imports* lists the imports, newest first. **Undo** (`POST /api/v1/library/imports/{id}/undo`) gives every track what it had before that import: tags, fades, markers, categories and labels it added, enabled or disabled, date added. Categories and labels the import created stay; delete them from their pages if they are no longer wanted. An import is undone once.

## API

| | |
|---|---|
| `POST /library/import/preview` | `{"rows": [ … ]}`: each row's status (`matched`, `not_found`, `duplicate_in_export`, `duplicate_in_library`, `invalid`), track, values to set, values set in Heron, conflicts and warnings; the export's categories and problems |
| `POST /library/import/apply` | `{"categories": [{"id", "name", "fx_player", "disable_tracks"}], "problems": [{"id", "action", "label"}], "rows": [{"track_id", "values"}]}`: `action` is `ignore`, `label` or `label_and_disable`; a missing value leaves Heron's |
| `GET /library/imports` | The last 50 imports |
| `POST /library/imports/{id}/undo` | Undoes an import (`409` if it was undone already) |

Admins only. Request bodies up to 64 MiB, 50,000 rows. The web container's nginx lets these two routes through up to that size (`docker/nginx.conf`); a reverse proxy of your own in front of Heron needs the same (`client_max_body_size 65m` for nginx), or it answers `413`.
