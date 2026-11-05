-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The play ratio: an exact quota of plays per track (docs/rotation.md#play-ratio).
-- `play_ratio` is in tenths, 1 to 10; NULL is 10, which changes nothing.
-- It stays while the track is disabled: re-enabling offers it again.
-- It is configuration (in backups, with the rest of `track_programming`).

ALTER TABLE track_programming
    ADD COLUMN play_ratio TINYINT UNSIGNED NULL AFTER sound_code,
    ADD CONSTRAINT track_programming_play_ratio CHECK (play_ratio BETWEEN 1 AND 10);

-- Each station's counter per track, 10 down to 1 (no row: 10). State, not
-- configuration: not in backups.
CREATE TABLE track_ratio_counters (
    station_id  BIGINT UNSIGNED NOT NULL,
    track_id    BIGINT UNSIGNED NOT NULL,
    counter     TINYINT UNSIGNED NOT NULL,
    PRIMARY KEY (station_id, track_id),
    KEY track_ratio_counters_track (track_id),
    CONSTRAINT track_ratio_counters_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT track_ratio_counters_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
