-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The Featured artist (docs/listener-interaction.md#featured-artist): the
-- artist of each week (from its Monday) or each day, per station. Both
-- plannings are kept: the show's settings say which one plays. A year
-- after its week or day is over, an entry is deleted.

CREATE TABLE featured_artists (
    station_id  BIGINT UNSIGNED NOT NULL,
    period      ENUM('week', 'day') NOT NULL,
    -- The Monday of the week, or the day.
    day         DATE NOT NULL,
    artist_id   BIGINT UNSIGNED NOT NULL,
    PRIMARY KEY (station_id, period, day),
    KEY featured_artists_artist (artist_id),
    CONSTRAINT featured_artists_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT featured_artists_artist FOREIGN KEY (artist_id) REFERENCES artists (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
