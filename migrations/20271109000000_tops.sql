-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Tops (docs/listener-interaction.md#tops): several charts per station,
-- each with its period, score and show. The single weekly Top 30 becomes
-- the first top of each station that had set it up, with its history.

-- A station's tops, in order. Their settings (JSON) are in the
-- configuration backups.
CREATE TABLE tops (
    id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
    station_id  BIGINT UNSIGNED NOT NULL,
    position    INT UNSIGNED NOT NULL DEFAULT 0,
    settings    JSON NOT NULL,
    KEY tops_by_station (station_id, position, id),
    CONSTRAINT tops_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- A top's chart, named by the day it is frozen (station time). A row is
-- written when it is frozen, or when a programmer chooses its new release
-- first.
CREATE TABLE top_charts (
    top_id                  BIGINT UNSIGNED NOT NULL,
    chart_key               DATE NOT NULL,
    from_at                 DATETIME(6) NOT NULL,
    to_at                   DATETIME(6) NOT NULL,
    -- When it was frozen; NULL while it runs.
    frozen_at               DATETIME(6) NULL,
    -- Votes on the eligible tracks over the period, at the freeze.
    votes                   BIGINT UNSIGNED NOT NULL DEFAULT 0,
    -- The most played eligible track's plays; NULL for the Top 30's
    -- charts, ranked on likes only.
    max_plays               BIGINT UNSIGNED NULL,
    -- The score's share of plays, the minimum votes and the size, as
    -- they were at the freeze.
    plays_percent           TINYINT UNSIGNED NOT NULL DEFAULT 0,
    min_votes               BIGINT UNSIGNED NOT NULL DEFAULT 0,
    size                    SMALLINT UNSIGNED NOT NULL DEFAULT 30,
    -- Chosen by hand; NULL: the proposed one.
    new_release_track_id    BIGINT UNSIGNED NULL,
    PRIMARY KEY (top_id, chart_key),
    CONSTRAINT top_charts_top FOREIGN KEY (top_id) REFERENCES tops (id) ON DELETE CASCADE,
    CONSTRAINT top_charts_new_release FOREIGN KEY (new_release_track_id) REFERENCES tracks (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- A frozen chart's ranks. The artist and title are kept as they were,
-- for a track deleted since.
CREATE TABLE top_entries (
    top_id          BIGINT UNSIGNED NOT NULL,
    chart_key       DATE NOT NULL,
    `rank`          SMALLINT UNSIGNED NOT NULL,
    track_id        BIGINT UNSIGNED NULL,
    artist          VARCHAR(512) NULL,
    title           VARCHAR(512) NULL,
    likes           BIGINT UNSIGNED NOT NULL,
    dislikes        BIGINT UNSIGNED NOT NULL,
    plays           BIGINT UNSIGNED NOT NULL,
    score           DOUBLE NOT NULL DEFAULT 0,
    movement        ENUM('up', 'down', 'same', 'new', 'reentry') NOT NULL,
    previous_rank   SMALLINT UNSIGNED NULL,
    -- The top's charts it was in, this one included.
    charts          SMALLINT UNSIGNED NOT NULL,
    PRIMARY KEY (top_id, chart_key, `rank`),
    KEY top_entries_by_track (top_id, track_id, chart_key),
    CONSTRAINT top_entries_chart FOREIGN KEY (top_id, chart_key) REFERENCES top_charts (top_id, chart_key) ON DELETE CASCADE,
    CONSTRAINT top_entries_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Tracks a programmer took out of a chart, and who.
CREATE TABLE top_removals (
    top_id          BIGINT UNSIGNED NOT NULL,
    chart_key       DATE NOT NULL,
    track_id        BIGINT UNSIGNED NOT NULL,
    removed_by      BIGINT UNSIGNED NULL,
    removed_at      DATETIME(6) NOT NULL,
    PRIMARY KEY (top_id, chart_key, track_id),
    CONSTRAINT top_removals_top FOREIGN KEY (top_id) REFERENCES tops (id) ON DELETE CASCADE,
    CONSTRAINT top_removals_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE,
    CONSTRAINT top_removals_user FOREIGN KEY (removed_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Each show's programme, built when it starts: the chart it plays (NULL
-- without one), its items in order (JSON), and how many are queued. Its
-- slot also keeps the plays in it out of every top's charts.
CREATE TABLE top_shows (
    top_id          BIGINT UNSIGNED NOT NULL,
    station_id      BIGINT UNSIGNED NOT NULL,
    opens_at        DATETIME(6) NOT NULL,
    closes_at       DATETIME(6) NOT NULL,
    chart_key       DATE NULL,
    programme       JSON NOT NULL,
    next_item       INT UNSIGNED NOT NULL DEFAULT 0,
    built_at        DATETIME(6) NOT NULL,
    PRIMARY KEY (top_id, opens_at),
    KEY top_shows_by_station (station_id, opens_at),
    KEY top_shows_by_chart (top_id, chart_key),
    CONSTRAINT top_shows_top FOREIGN KEY (top_id) REFERENCES tops (id) ON DELETE CASCADE,
    CONSTRAINT top_shows_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- The Top 30 of each station that set it up (its settings, its hours or
-- a chart), with its settings in the new shape: the week reset at its
-- close, 100 % likes, its first range of hours as its show.
CREATE TEMPORARY TABLE top30_old AS
SELECT s.id AS station_id,
       CAST(t.value AS CHAR) AS top,
       CAST(h.value AS CHAR) AS shown,
       CAST(COALESCE(JSON_VALUE(t.value, '$.close_day'), 7) AS UNSIGNED) AS close_day,
       CAST(COALESCE(JSON_VALUE(t.value, '$.close_hour'), 17) AS UNSIGNED) AS close_hour,
       CAST(COALESCE(JSON_VALUE(t.value, '$.size'), 30) AS UNSIGNED) AS size
FROM stations s
LEFT JOIN station_settings t ON t.station_id = s.id AND t.name = 'top30'
LEFT JOIN station_settings h ON h.station_id = s.id AND h.name = 'show_top30'
WHERE t.value IS NOT NULL OR h.value IS NOT NULL
   OR EXISTS (SELECT 1 FROM top30_charts c WHERE c.station_id = s.id);

INSERT INTO tops (station_id, position, settings)
SELECT o.station_id, 0, JSON_OBJECT(
    'name', 'Top 30',
    'enabled', COALESCE(JSON_EXTRACT(o.shown, '$.enabled'), JSON_EXTRACT('true', '$')),
    'size', CASE WHEN o.size <= 3 THEN 3 WHEN o.size <= 5 THEN 5 WHEN o.size <= 10 THEN 10
                 WHEN o.size <= 20 THEN 20 WHEN o.size <= 30 THEN 30 WHEN o.size <= 40 THEN 40
                 ELSE 50 END,
    'period', 'week',
    'reset_weekday', IF(o.close_hour >= 24, o.close_day % 7 + 1, o.close_day),
    'reset_day', 1,
    'reset_time', IF(o.close_hour >= 24, '00:00', CONCAT(LPAD(o.close_hour, 2, '0'), ':00')),
    'plays_percent', 0,
    'min_votes', CAST(COALESCE(JSON_VALUE(o.top, '$.min_votes'), 10) AS UNSIGNED),
    'category_ids', COALESCE(JSON_EXTRACT(o.top, '$.category_ids'), JSON_ARRAY()),
    'jingle_category_id', JSON_EXTRACT(o.top, '$.jingle_category_id'),
    'new_release_category_id', JSON_EXTRACT(o.top, '$.new_release_category_id'),
    'hide_unplayed', JSON_EXTRACT('true', '$'),
    'slot', IF(JSON_EXTRACT(o.shown, '$.hours[0]') IS NULL, NULL, JSON_OBJECT(
        'every', IF(JSON_LENGTH(JSON_EXTRACT(o.shown, '$.hours[0].days')) >= 7, 'day', 'week'),
        'weekday', IF(JSON_LENGTH(JSON_EXTRACT(o.shown, '$.hours[0].days')) >= 7, NULL,
                      CAST(JSON_VALUE(o.shown, '$.hours[0].days[0]') AS UNSIGNED)),
        'nth', NULL,
        'start', CONCAT(LPAD(JSON_VALUE(o.shown, '$.hours[0].start_hour'), 2, '0'), ':00'),
        'end', CONCAT(LPAD(JSON_VALUE(o.shown, '$.hours[0].end_hour'), 2, '0'), ':00'))),
    'website', COALESCE(JSON_EXTRACT(o.shown, '$.website'), JSON_OBJECT()),
    'notifications', COALESCE(JSON_EXTRACT(o.shown, '$.notifications'), JSON_OBJECT()))
FROM top30_old o;

-- A week's key: the day it closed, in station time.
CREATE TEMPORARY TABLE top30_keys AS
SELECT tp.id AS top_id, o.station_id,
       o.close_day - 1 + IF(o.close_hour >= 24, 1, 0) AS days, o.size
FROM top30_old o JOIN tops tp ON tp.station_id = o.station_id;

INSERT INTO top_charts (top_id, chart_key, from_at, to_at, frozen_at, votes, max_plays,
                        plays_percent, min_votes, size, new_release_track_id)
SELECT k.top_id, DATE_ADD(c.week, INTERVAL k.days DAY), c.from_at, c.closes_at, c.frozen_at,
       c.votes, NULL, 0, CAST(JSON_VALUE(tp.settings, '$.min_votes') AS UNSIGNED), k.size,
       c.new_release_track_id
FROM top30_charts c
JOIN top30_keys k ON k.station_id = c.station_id
JOIN tops tp ON tp.id = k.top_id;

INSERT INTO top_entries (top_id, chart_key, `rank`, track_id, artist, title, likes, dislikes,
                         plays, score, movement, previous_rank, charts)
SELECT k.top_id, DATE_ADD(e.week, INTERVAL k.days DAY), e.`rank`, e.track_id, e.artist, e.title,
       e.likes, e.dislikes, e.plays,
       IF(e.likes + e.dislikes = 0, 0, e.likes / (e.likes + e.dislikes)),
       e.movement, e.previous_rank, e.weeks
FROM top30_entries e JOIN top30_keys k ON k.station_id = e.station_id;

INSERT INTO top_removals (top_id, chart_key, track_id, removed_by, removed_at)
SELECT k.top_id, DATE_ADD(r.week, INTERVAL k.days DAY), r.track_id, r.removed_by, r.removed_at
FROM top30_removals r JOIN top30_keys k ON k.station_id = r.station_id;

INSERT INTO top_shows (top_id, station_id, opens_at, closes_at, chart_key, programme,
                       next_item, built_at)
SELECT k.top_id, s.station_id, s.opens_at, s.closes_at,
       IF(s.week IS NULL, NULL, DATE_ADD(s.week, INTERVAL k.days DAY)), s.programme,
       s.next_item, s.built_at
FROM top30_shows s JOIN top30_keys k ON k.station_id = s.station_id;

DROP TEMPORARY TABLE top30_keys;
DROP TEMPORARY TABLE top30_old;

DELETE FROM station_settings WHERE name IN ('top30', 'show_top30');

DROP TABLE top30_shows;
DROP TABLE top30_removals;
DROP TABLE top30_entries;
DROP TABLE top30_charts;
