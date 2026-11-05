# Library management

How Heron files, identifies and reorganizes music. Delivered in [M1b](roadmap/m1b-library-management.md) by `heron-librarian`, the only service with write access to the library.

```
uploads (heron-api) ──┐
                      ├──► buffer zone ──── import ────► music library ──► scanner, analyzer
incoming folder ──────┘    fingerprint, AcoustID,       (read-write for     (read-only)
  (drop zone)              MusicBrainz, flags,           heron-librarian
                           planned path                  only)
                                                         └─ .heron-trash/  deleted tracks
```

Like iTunes' "Keep Music folder organized": new files, uploaded from the browser or dropped into the incoming folder, wait in a **buffer zone** where they are identified and checked; once someone imports them they are tagged and filed under Artist / Album / Track. Tracks are deleted through a **trash** they can be restored from until it is purged. Nothing else in the library is touched unless you ask, and every change is journaled and can be undone.

## Folders and permissions

| Folder | Compose variable (host) | Mounted in |
|---|---|---|
| Music library | `HERON_MUSIC_DIR` | `heron-librarian` read-write; `heron-api`, `heron-analyzer`, `heron-engine` read-only |
| Incoming folder | `HERON_INCOMING_FOLDER` | `heron-librarian` only |
| Buffer zone | `HERON_STAGING_FOLDER` | `heron-api` (writes uploads) and `heron-librarian`, read-write |

`heron-librarian` and `heron-api` run as `HERON_LIBRARIAN_USER` (`uid:gid`), which must own the three folders: use the output of `id -u` and `id -g` for the owner of your music. The buffer zone is best on the library's file system, so imports are renames rather than copies.

## The incoming folder

- **When:** at startup, after file system changes (debounced 2 s; reads do not count), and every `HERON_INCOMING_INTERVAL` seconds (default 60). Set `HERON_INCOMING_WATCH=false` where watching does not work (some network shares).
- **What:** the same audio extensions as the scanner. Hidden files and other files (cover art, playlists, notes) are left in place.
- **Files still being copied:** files modified less than `HERON_IMPORT_SETTLE` seconds ago (default 5) wait. A file that changes while it is being imported is left for the next pass.
- **Siblings:** the files of one folder form a **group** in the buffer zone, so the files of an album agree on their release.
- Settled files **move into the buffer zone** (a rename, or a verified copy across file systems), recorded in `staged_files` before they are touched, and wait there with uploads until someone imports them. Nothing reaches the library before.
- Emptied folders are removed from the incoming folder.

## The buffer zone

New files wait in `HERON_STAGING_FOLDER`, one folder per group, until they are imported or discarded. The web UI's **Import music** page shows them; files never expire.

**Uploads** (web UI, or `POST /api/v1/staging/…`, see [API](api.md#buffer-zone)): the browser creates a group, announces each file (refused when it is not one of the scanner's audio extensions, when it is larger than `max_upload_mb`, or when it would leave less than `min_free_mb` free on the disk), then sends it in chunks of at most 16 MiB at the offset the server has. After a dropped connection it asks how far the server got and carries on from there. A file is written under a hidden temporary name and shows as complete only once its last byte has arrived. Uploads that make no progress for a day are given up.

**Identification and checks:** once a group's files are complete (no upload has made progress for 10 minutes, or none is left), `heron-librarian` reads each file's tags and duration, fingerprints it and identifies the group as described below. Each file then shows the tags it would be written with, its cover (from the Cover Art Archive), the path the [layout](#layout) gives it today, and its candidate releases. Files are **flagged** when:

| Flag | |
|---|---|
| In the library | The same file (content hash) or the same audio (fingerprint) is a library track |
| Twice here | The same file or audio waits in the buffer zone too |
| Same recording | A library track is identified as the same MusicBrainz recording |
| Unreadable | The file cannot be decoded |
| Too short | Too short to be fingerprinted (and identified) |
| Unsupported | An audio format Heron cannot decode |

**Choices:** for a file, pick another of its candidate releases, or keep its own tags; the librarian works out the tags and the path again. Confident matches and files without anything to look at are selected already, so a clean album is imported in one click.

**Import:** a library task (`staging_import`) files the selected ready files like any import: tags written, filed where the layout says, `cover.jpg` saved, as one `import` batch that can be undone, then analyzed. Files picked by hand are recorded as identified, files that kept their own tags as reviewed (`kept`), and files imported while they needed review wait on the review screen.

**Edit tags:** on a file or a selection, set or remove title, artist, album, album artist, year, track and disc. Edits (`tag_edits`) go on top of the tags the file gets otherwise (`base_tags`: from its identification, a release picked by hand or its own tags), so picking another release keeps them; the librarian works the path out again without looking the release up. They are written into the file on import, like an identified file's tags, in the same batch.

**Prepare:** on a file, a selection or a group, set what its track gets once imported: categories (with a weight), labels, mood and sound code. Files can be prepared until their import starts. Preparing changes neither the library nor the file; it is kept with the file in the database (`staged_file_categories`, `staged_file_labels`, `staged_files.mood` and `sound_code`). The import gives the track all of it in the transaction that creates it, so no track is ever in the library without its preparation, even after a crash. Categories or labels deleted in the meantime are left out.

**Discard:** the file is removed from the buffer zone, with what it was prepared with. It never reached the library.

**Finding imported tracks:** when an import ends, the Import music page links to its tracks in the library (`GET /tracks?batch_id=`). Their date added is the time of the import.

## Identification

Identification needs the station's own **AcoustID application key** (free keys are for non-commercial use: commercial stations need a commercial key from AcoustID). Without a key, incoming files are filed using their own tags, and nothing is flagged for review.

1. **Fingerprint:** Chromaprint of the first two minutes, like `fpcalc` (`rusty-chromaprint`). The analyzer also fingerprints every track during its analysis decode (`track_fingerprints`), so existing tracks can be identified later without reading them again.
2. **AcoustID** returns the recordings that match, with a score, and the releases they are on.
3. **The release is picked from the siblings:** the release matching the most files of the folder is picked first, then the next one for the files left. A pick is applied only when it is unambiguous:
   - one release matches more files than any other; or
   - exactly one of the tied releases is titled like the files' album tag; or
   - the tied releases are editions of one release group that put every file at the same disc and track (the tags are the same whichever is picked; the earliest is).
4. **MusicBrainz** gives the release's tags, once per release (not per file).
5. **Cover Art Archive:** the release's front cover (500 px), saved as `cover.jpg` in the album folder unless one is there already.

A match is **confident** when its AcoustID score is at least `min_score` (0.9) and it points to a single release: the file is imported with its tags. Otherwise the file keeps its own tags, unless a candidate is picked in the buffer zone, and is marked **needs review**, with the reason and the candidate releases (`track_identifications` once imported):

| Reason | |
|---|---|
| `not found on AcoustID` | Unknown recording |
| `best AcoustID score … is below …` | A doubtful match |
| `matches N releases, none clearly better` | A lone file on an album and a compilation, for example |
| `the audio could not be fingerprinted` | Undecodable, or too short |
| `identification failed: …` | AcoustID or MusicBrainz unreachable three times (the folder waits 1, then 2 minutes between tries) |

Tracks imported while they need review wait on the **review screen** of the web UI (*To organize*; `heron-librarian review` lists them too). For each track, pick one of its candidate releases, or keep the file's own tags (`kept`). A picked release is applied by the librarian like an import: MusicBrainz tags written into the file, filed where the layout says, cover saved, as a `review` batch that can be undone (the track then needs review again).

**Rate limits:** one request per second to MusicBrainz and to the Cover Art Archive, three per second to AcoustID, with a User-Agent that identifies Heron (`HeronBroadcaster/<version> ( https://heronbroadcaster.org )`). Each request waits a full interval after the previous response, so requests never arrive closer than the limit, even when one was delayed on the way. A `503` or `429` answer slows down and retries.

## Tags

Written into identified files:

| Tag | Value |
|---|---|
| Title, artist | The track on the release (artist credit as printed) |
| Album, album artist | The release |
| Track, track total, disc, disc total | The position on the release |
| Date | The release group's first release date (the original year, not the reissue's) |
| Compilation | Set when the album artist is Various Artists |
| MusicBrainz IDs | Recording, release track, release, release group, artist, album artist (Picard's tag names) |

Fields MusicBrainz leaves empty keep the file's value. Other tags (genre, comments, lyrics, …) are never touched. Cover art is embedded only when `embed_cover` is on, and only in files without one.

The previous tags are kept in `track_tag_changes`, so a change can be undone. Re-tagging keeps the track's ID, and so its analysis, categories and history: the audio did not change.

### Editing tags from the browser

Title, artist, album, album artist, year, genre, track and disc numbers can be edited from the web UI, on one track or a selection: a field set on a selection takes the same value on every track, fields left alone keep each track's value. The programming details (ISRC, record label, BPM, mood) can be written into the files too, each track's own. Only those fields are written: comments, lyrics, pictures, ReplayGain and the others are kept as they are.

The librarian writes them as an `edit` batch, in place: the file is not moved (organizing files it by its new tags, after its dry run) and is not analyzed again. A file changed since the last scan is skipped until the scanner has read it.

### Keeping tags in Heron instead of the files

With the library setting `write_tags` off (*Settings › Library*, admins), the tag editor saves its fields in Heron's database (`track_tag_overrides`) and leaves the files as they are: for a library shared with other tools, synchronized, backed up by hash, or kept as masters. Only the tags of the `tracks` table are kept this way (title, artist, album, album artist, year, genre, track and disc numbers); the programming details are in Heron already.

- The values show at once everywhere Heron uses a track's tags: the library, rotation and its separation rules, history, now-playing, webhooks and the stream's metadata.
- The scanner, and the librarian when it writes a file, apply them on top of the tags they read, so they survive a rescan or a file changed on disk. Organizing the library files tracks by them too.
- A track's page lists its overridden fields, with **Back to the file's tags** (`POST /tracks/tag-overrides/clear`), which drops them and reads the file again.
- Turned back on, `write_tags` writes edits into the files again; a field written drops its override, the file has the value now. Edits queued before the setting was turned off fail without touching the files.
- Imports from the buffer zone and the review screen still write tags into the files they file: `write_tags` is about tags edited on tracks of the library.

### Undoing byte for byte

Every tag write (imports, reviews, edits, undos) also records how to rebuild the file as it was from the file it wrote, in `track_tag_changes.restore`: the original's bytes as ranges of the new file (the audio, which a tag change does not touch, even shifted) and the bytes that differ (the old tags), found like rsync does. Undoing the change rebuilds the original from it, checked against the original's hash, so the file gets back the same bytes, not just the same tags.

When the file changed again since (another edit not undone), or for changes recorded before M5b, and for files over 256 MiB, undoing writes the previous tags back instead: the same tags, maybe not the same bytes.

## MusicBrainz details

For identified tracks (whose tags carry a MusicBrainz recording and release), the librarian looks up the release on MusicBrainz (`inc=labels recordings isrcs genres release-groups`, one request a second): album, record label and catalog number, ISRC, the year of first release (the earlier of the recording's and the release group's) and genres, most voted first. They are saved in `track_musicbrainz`, not in the files (they can be written there from the tag editor).

The whole library is looked up on request (*Settings*, or `POST /api/v1/library/musicbrainz-details`); once a lookup has finished, tracks without details (new tracks) are looked up in the background, up to 30 releases a minute. A release MusicBrainz does not know is saved empty and not looked up again unless asked for every track.

## Layout

Files are filed at `layout` under the library root. Default:

```
{album_artist}/{album}/{disc-}{track:02} {title}.{ext}
```

| Field | Value |
|---|---|
| `{album_artist}` | Album artist, else artist. `Compilations` for various-artists albums |
| `{artist}` | Artist, else album artist |
| `{album}`, `{title}`, `{year}`, `{ext}` | `{title}` defaults to the original file name |
| `{track}`, `{disc}` | Numbers; `{track:02}` pads to two digits |
| `{disc-}` | `2-` on multi-disc albums, nothing otherwise |

Missing artists and albums give `Unknown Artist` and `Unknown Album`. Names are made safe on every file system: `/ \ : * ? " < > |` and control characters become `_`, trailing dots and spaces are removed, a leading dot becomes `_` (hidden files are not imported), Windows reserved names (`CON`, `NUL`, …) get a `_`, and each component is at most 255 bytes.

**Files are never overwritten:** a name already taken (by a file, or by a track whose file is missing) gets ` (2)`, ` (3)`, … before the extension.

## Moves and the journal

Every move and tag write is recorded in `library_moves` before the file is touched, and the track is updated in the same step that marks the file in place, so the scanner never sees a track's file go missing:

| Step | Status |
|---|---|
| Record the operation (an imported file also gets its track, marked missing until the file is in place) | `pending` |
| Move the file without ever replacing another: a rename (`renameat2(RENAME_NOREPLACE)`, or a hard link and unlink where that is not supported); across file systems, or when tags change, a copy into a hidden `.heron-<id>` file next to the target, flushed and read back to verify, then renamed into place | |
| Point the track at the file, with its new stamp | `moved` (copy) or `done` (rename) |
| Remove the original of a copy | `done` |

On startup, before anything else, interrupted operations are **finished or rolled back**: a file that reached its new path (recognized by its content hash, or by being the same file) is recorded there; otherwise the operation is abandoned and the file stays where it was. The scanner skips files of an unfinished operation, and a [library lock](#coordination-with-the-scanner) keeps it from scanning in the middle of one.

The one replacement Heron makes is re-tagging a file in place: the re-tagged copy atomically replaces the original, which never leaves a half-written file.

## Reorganizing an existing library

Only on request, after a dry run: from the web UI (*To organize*, admins), or on the command line:

```sh
docker compose exec heron-librarian heron-librarian organize           # dry run: lists every move, changes nothing
docker compose exec heron-librarian heron-librarian organize --apply   # moves, as one batch
docker compose exec heron-librarian heron-librarian undo <batch>       # back to the previous paths
docker compose exec heron-librarian heron-librarian batches            # recent batches
```

Organizing uses the files' current tags (it does not identify them). Files that are not tracks (cover art, playlists) stay where they are; emptied folders are removed.

**Undo** moves files back to their previous paths and gives them back their previous tags, byte for byte (see [Undoing byte for byte](#undoing-byte-for-byte)). Undoing an import restores the files' own tags; the files stay in the library. A file whose previous path has been taken since is left where it is and reported. An undo interrupted by a crash is resumed by running it again. An organization interrupted by a crash leaves a partial batch: run `organize --apply` again for the rest, and undo both batches to go back.

## Deleting tracks and the trash

Tracks are deleted from the web UI (*Tracks* and a track's page; operators and admins), as a library task (`delete`):

- `heron-librarian` moves each file to `.heron-trash/<batch>/<its path>` at the library root (a rename: same file system; hidden, so the scanner skips it), journaled as a `delete` batch. The track is marked deleted in the step that records the file in the trash, so a track is never in the trash without its file, nor out of it with its file there.
- A deleted track leaves the library, rotation and its categories (it is marked missing too). The queue manager takes it out of the queue within seconds, as it does for missing files; a track on air plays to its end, its file is already open.
- Tracks whose file is missing can be deleted too: only their record goes to the trash.
- Files that are not tracks (`cover.jpg`, playlists) go to the trash with the last track of their folder; emptied folders are removed.
- Its analysis, categories, overrides, identification and history are kept, so **restoring** it (undoing the batch, from the *Trash* page) brings it back whole, its file back where it was. A path taken since is reported and that track stays in the trash.
- After `trash_days` (30 by default), the librarian **purges** the trash (it looks once an hour): each track is deleted, then its file. Their plays keep their artist and title (`play_history.track_id` becomes NULL); restrictions on them go with them. Admins can also **empty the trash** at once (`empty_trash`). An interrupted purge leaves no track without its file: at worst a file waits in the trash for the next purge.

## Settings

Stored as JSON under the `library` name in the `settings` table; missing fields take the defaults. Change them through the API (`PATCH /api/v1/settings/library` with the fields to change, admins only; the AcoustID key is never sent back, see [API](api.md)), or on the command line:

```sh
docker compose exec heron-librarian heron-librarian settings
docker compose exec heron-librarian heron-librarian settings --set '{"acoustid_key": "…"}'
```

| Setting | Default |
|---|---|
| `acoustid_key` | none: identification off |
| `layout` | `{album_artist}/{album}/{disc-}{track:02} {title}.{ext}` |
| `min_score` | 0.9 |
| `save_cover` | true |
| `embed_cover` | false |
| `max_upload_mb` | 2048: largest file accepted by upload |
| `min_free_mb` | 5120: uploads that would leave less free space on the buffer zone's disk are refused |
| `trash_days` | 30: days deleted tracks stay in the trash |
| `write_tags` | true: tags edited from the browser are written into the files; false keeps them in Heron's database ([above](#keeping-tags-in-heron-instead-of-the-files)) |

## Work asked through the API

The web UI's review, organize, undo, tag edits, MusicBrainz lookups, imports from the buffer zone, deletions, emptying the trash and the rendering of [medleys](medleys.md) go through the API, which cannot write to the library: it queues **library tasks** (`library_tasks`) that the librarian runs one at a time, in order, next to the buffer zone's identification (both take the library lock for each group of file operations). Results (the dry run's moves, the batch report) are stored with the task for the web UI to show. A task running when the librarian stops is marked failed: ask again (an undo resumes where it stopped; an organization is applied once more for the rest; files of an interrupted import are ready again, or imported if they made it). See [API – Library management](api.md#library-management).

## Coordination with the scanner

The scanner (in `heron-api`, read-only) and the librarian share a MariaDB named lock (`GET_LOCK`): the scanner holds it for a scan, the librarian for each group of file operations. A process that dies releases it with its connection. Since the librarian records the stamp of every file it writes, the scanner finds them unchanged and does not re-import them.

## Limitations

- The engine holds its upcoming tracks by path. The queue manager follows moves within 5 seconds ([API](api.md#queue-manager)), so only a track due to start in those seconds can be skipped.
- Existing tracks are fingerprinted but not identified yet: identification runs on new files only.
- Duplicates are flagged, not refused: importing one gives it a ` (2)` suffix.
- An upload resumes after a dropped connection, not after the page is reloaded: the file starts over (the abandoned one is given up after a day).
- Restoring works on a whole deletion (batch), not on one track of it.

Metrics: `heron_librarian_staged_total{source}`, `heron_librarian_imports_total{result}`, `heron_api_uploads_total`.
