-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- A track's own daypart separation (docs/rotation.md#daypart-separation):
-- `dayparts` (0 to 10) and `daypart_days` (0 to 7) replace its categories'
-- values, in every category and on every station; NULL follows them.
-- Configuration: in backups, with the rest of `track_programming`.

ALTER TABLE track_programming
    ADD COLUMN dayparts TINYINT UNSIGNED NULL AFTER play_ratio,
    ADD COLUMN daypart_days TINYINT UNSIGNED NULL AFTER dayparts,
    ADD CONSTRAINT track_programming_dayparts CHECK (dayparts <= 10),
    ADD CONSTRAINT track_programming_daypart_days CHECK (daypart_days <= 7);
