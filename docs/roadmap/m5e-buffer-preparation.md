# M5e – Preparing tracks in the buffer zone

**Depends on:** M5c (the buffer zone) and M5b (labels, mood and sound code).

New music is checked in the buffer zone ([M5c](m5c-track-management.md)), but everything a programmer does next happens after the import: find the new tracks in the library, then put them in categories, label them, set their mood and sound code. Feedback from the first station using the buffer: tracks should be prepared **from the buffer page**, so they are ready to air the moment they are imported, tags included; and the tracks just imported are hard to find in the library.

## Scope

### Preparing files

- On a file, a selection or a whole group of the buffer, before it is imported:
  - put it in **categories** (with a weight, 1 by default), or take it out;
  - add or remove **labels**;
  - set or clear its **mood** and **sound code** (on the station's scales).
- Files can be prepared from the moment they show in the buffer, while they are identified, and until they are imported; not while they are being imported.
- The buffer shows what each file has been given, and its group sums it up (categories and labels that all, or some, of its files have).
- Preparing a file changes nothing in the library or in the file: it is kept in the database with the file, and dropped if the file is discarded.

### Editing tags

- Edit title, artist, album, album artist, year, track and disc number of a file, or of a selection (a field set on a selection applies to all of it; fields left alone keep each file's value). This was deferred from M5c.
- Edits go on top of the tags the file gets from its identification, a release picked by hand or its own tags: picking another release keeps them. The buffer shows the edited fields, and the path the layout gives the file is worked out again.
- They are written into the file when it is imported, like the tags of an identified file, in the same `import` batch (undone byte for byte).

### Importing a prepared file

- The track an import creates gets the file's categories, labels, mood and sound code in the same step that creates it: there is no moment, even after a crash, when a track is in the library without its preparation.
- Categories or labels deleted in the meantime are left out; the rest applies.
- Undoing the import removes the tracks with what they were given, like any imported track.

### Finding imported tracks

- The library can be filtered by **import batch**: the tracks an import created.
- When an import ends, the buffer page links to its tracks in the library. The filter shows in the library as a chip that clears it.
- *Date added* on tracks imported from the buffer is the time of the import.

## Acceptance criteria

- [x] A group of files uploaded to the buffer is put in two categories, given a label and a mood, one of its files taken out of a category, and imported: each track is in the library with exactly what its file was given, as soon as it exists.

  `librarian/tests/preparation.rs` (three files, one taken out of a category, one with another weight), the `api` end-to-end test (uploaded, prepared and imported through the API), and the Buffer page against a running station (two albums from the incoming folder, prepared and imported from Chrome).
- [x] Tags edited on a selection of identified files, then on one of them, show in the buffer with the new planned path, without looking the release up again; once imported, the files carry them and the import is undone byte for byte.

  `librarian/tests/preparation.rs` against a mock of AcoustID and MusicBrainz (no MusicBrainz request after the edits), and the `api` end-to-end test (fields that cannot be edited and invalid years refused, an edit undone).
- [x] A file discarded after being prepared leaves nothing behind; a category deleted after a file was put in it is left out at import, the rest applies.
- [x] Preparing a file while it is being imported is refused; interrupting the librarian while it imports prepared files (chaos test) leaves no track without its preparation.

  `api` end-to-end test (`409`), and `librarian/tests/chaos.rs`, which now prepares its 20 files and checks every track after each crash and kill.
- [x] The buffer page links to the tracks of the import that just ended; the library filtered by that batch lists exactly them, and sorted by date added lists them first.

## Results

- **Preparation** ([Library management](../library.md#the-buffer-zone)): `staged_file_categories`, `staged_file_labels`, `staged_files.mood` and `sound_code`, set with `POST /staging/files/preparation`; the journal copies them to the track in the transaction that inserts it. The API locks the files while it prepares them, and the librarian claims files for an import with an `UPDATE`, so a file is prepared either before its import starts or not at all. Discarding a file drops its preparation.
- **Tag editing**: `staged_files.tag_edits` on top of `base_tags` (what identification, a picked release or the own tags give); `POST /staging/files/tags` sets, removes or resets fields, and the librarian works the tags and path out again without a lookup. Picking a release clears `base_tags`, so the release's tags are worked out with the edits on top.
- **Finding imported tracks**: `GET /tracks?batch_id=`; the Import music page links to `/library?batch=…&sort=added` when an import ends, and the library shows the filter as a chip.
- **Date added**: no bug was found in sorting or filtering by date added; tracks imported from the buffer get the time of their import. Files waiting in the buffer are not tracks yet, which is likely why they could not be found among the tracks.
- **Web UI**: the Category, Labels and Mood dialogs of the library work on buffer files; an *Edit tags* dialog for a selection or a file; prepared values as chips on files and groups; edited fields marked, with *Undo the edit*.
- Not in the buffer: genre and the programming details written into files (ISRC, record label, BPM), which need the track; the BPM is measured after the import.

## References

- [Library management](../library.md#the-buffer-zone)
- [Web UI](../web.md)
- [Data model](../data-model.md)
