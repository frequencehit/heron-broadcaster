-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Play ratios per station (docs/rotation.md#play-ratio): the track's
-- `track_programming.play_ratio` is its default, and a station can have its
-- own. Who disabled a track, and when, everywhere or on a station.
-- Configuration: in backups.

ALTER TABLE tracks
    ADD COLUMN disabled_by BIGINT UNSIGNED NULL AFTER enabled,
    ADD COLUMN disabled_at DATETIME(6) NULL AFTER disabled_by,
    ADD CONSTRAINT tracks_disabled_by FOREIGN KEY (disabled_by) REFERENCES users (id) ON DELETE SET NULL;

-- A station's own ratio for a track (no row: the default). `play_ratio` in
-- tenths, 1 to 10, NULL for the default; `enabled` off disables the track
-- on that station only, which keeps its ratio there (a ratio of 0).
CREATE TABLE track_station_ratios (
    track_id     BIGINT UNSIGNED NOT NULL,
    station_id   BIGINT UNSIGNED NOT NULL,
    play_ratio   TINYINT UNSIGNED NULL,
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    disabled_by  BIGINT UNSIGNED NULL,
    disabled_at  DATETIME(6) NULL,
    PRIMARY KEY (track_id, station_id),
    KEY track_station_ratios_station (station_id),
    CONSTRAINT track_station_ratios_play_ratio CHECK (play_ratio BETWEEN 1 AND 10),
    CONSTRAINT track_station_ratios_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE,
    CONSTRAINT track_station_ratios_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT track_station_ratios_disabled_by FOREIGN KEY (disabled_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
