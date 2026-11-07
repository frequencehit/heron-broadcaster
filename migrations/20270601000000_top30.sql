-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Top 30 (docs/listener-interaction.md#top-30): the weekly chart of the
-- most liked tracks, played on air. Not in the configuration backups:
-- with the history, in the database dump.

-- A station's week: from Monday 00:00 to its close. A row is written when
-- the week is frozen, or when a programmer chooses its new release first.
CREATE TABLE top30_charts (
    station_id              BIGINT UNSIGNED NOT NULL,
    -- Its Monday, in the station's time zone.
    week                    DATE NOT NULL,
    from_at                 DATETIME(6) NOT NULL,
    closes_at               DATETIME(6) NOT NULL,
    -- When it was frozen; NULL while the week runs.
    frozen_at               DATETIME(6) NULL,
    -- Votes on the eligible tracks over the week, at the freeze.
    votes                   BIGINT UNSIGNED NOT NULL DEFAULT 0,
    -- Chosen by hand; NULL: the proposed one.
    new_release_track_id    BIGINT UNSIGNED NULL,
    PRIMARY KEY (station_id, week),
    CONSTRAINT top30_charts_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT top30_charts_new_release FOREIGN KEY (new_release_track_id) REFERENCES tracks (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- A frozen chart's ranks. The artist and title are kept as they were,
-- for a track deleted since.
CREATE TABLE top30_entries (
    station_id      BIGINT UNSIGNED NOT NULL,
    week            DATE NOT NULL,
    `rank`          SMALLINT UNSIGNED NOT NULL,
    track_id        BIGINT UNSIGNED NULL,
    artist          VARCHAR(512) NULL,
    title           VARCHAR(512) NULL,
    likes           BIGINT UNSIGNED NOT NULL,
    dislikes        BIGINT UNSIGNED NOT NULL,
    plays           BIGINT UNSIGNED NOT NULL,
    movement        ENUM('up', 'down', 'same', 'new', 'reentry') NOT NULL,
    previous_rank   SMALLINT UNSIGNED NULL,
    weeks           SMALLINT UNSIGNED NOT NULL,
    PRIMARY KEY (station_id, week, `rank`),
    KEY top30_entries_by_track (station_id, track_id, week),
    CONSTRAINT top30_entries_chart FOREIGN KEY (station_id, week) REFERENCES top30_charts (station_id, week) ON DELETE CASCADE,
    CONSTRAINT top30_entries_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Tracks a programmer took out of a week's chart, and who.
CREATE TABLE top30_removals (
    station_id      BIGINT UNSIGNED NOT NULL,
    week            DATE NOT NULL,
    track_id        BIGINT UNSIGNED NOT NULL,
    removed_by      BIGINT UNSIGNED NULL,
    removed_at      DATETIME(6) NOT NULL,
    PRIMARY KEY (station_id, week, track_id),
    CONSTRAINT top30_removals_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT top30_removals_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE,
    CONSTRAINT top30_removals_user FOREIGN KEY (removed_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Each show's programme, built when it starts: the chart it plays (NULL
-- without one), its items in order (JSON), and how many are queued.
CREATE TABLE top30_shows (
    station_id      BIGINT UNSIGNED NOT NULL,
    opens_at        DATETIME(6) NOT NULL,
    closes_at       DATETIME(6) NOT NULL,
    week            DATE NULL,
    programme       JSON NOT NULL,
    next_item       INT UNSIGNED NOT NULL DEFAULT 0,
    built_at        DATETIME(6) NOT NULL,
    PRIMARY KEY (station_id, opens_at),
    KEY top30_shows_by_week (station_id, week),
    CONSTRAINT top30_shows_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
