-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The match (docs/listener-interaction.md#match): every few tracks in the
-- show's hours, listeners vote between two tracks of one category for the
-- next one. Each match is kept for good, with its votes counted; each
-- device's vote is kept so that it can change, and deleted an hour after
-- the match closes. No IP address.
-- In the database dump of backups; the settings are in the configuration.

CREATE TABLE matches (
    id                 BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id         BIGINT UNSIGNED NOT NULL,
    status             ENUM('queued', 'open', 'closed', 'cancelled') NOT NULL DEFAULT 'queued',
    category_id        BIGINT UNSIGNED NULL,
    -- The track the vote runs during, its announcement, and the two tracks.
    vote_track_id      BIGINT UNSIGNED NULL,
    jingle_track_id    BIGINT UNSIGNED NULL,
    track_a_id         BIGINT UNSIGNED NULL,
    track_b_id         BIGINT UNSIGNED NULL,
    -- Their queue entries, for the engine started then.
    engine_started_at  BIGINT UNSIGNED NOT NULL,
    vote_entry_id      BIGINT UNSIGNED NOT NULL,
    entry_a_id         BIGINT UNSIGNED NOT NULL,
    entry_b_id         BIGINT UNSIGNED NOT NULL,
    -- The two tracks' picks in the rotation log.
    pick_a_id          BIGINT UNSIGNED NULL,
    pick_b_id          BIGINT UNSIGNED NULL,
    -- When the winner was planned to start: tracks are counted from it.
    planned_at         DATETIME(6) NOT NULL,
    created_at         DATETIME(6) NOT NULL,
    opened_at          DATETIME(6) NULL,
    closes_at          DATETIME(6) NULL,
    -- The next transition, as planned when the vote opened.
    transition_at      DATETIME(6) NULL,
    closed_at          DATETIME(6) NULL,
    votes_a            INT UNSIGNED NOT NULL DEFAULT 0,
    votes_b            INT UNSIGNED NOT NULL DEFAULT 0,
    winner             ENUM('a', 'b') NULL,
    decided_by         ENUM('votes', 'tie', 'no_votes', 'early') NULL,
    closed_by          BIGINT UNSIGNED NULL,
    PRIMARY KEY (id),
    KEY matches_station_status (station_id, status),
    KEY matches_station_planned (station_id, planned_at),
    CONSTRAINT matches_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT matches_category FOREIGN KEY (category_id) REFERENCES categories (id) ON DELETE SET NULL,
    CONSTRAINT matches_vote_track FOREIGN KEY (vote_track_id) REFERENCES tracks (id) ON DELETE SET NULL,
    CONSTRAINT matches_jingle_track FOREIGN KEY (jingle_track_id) REFERENCES tracks (id) ON DELETE SET NULL,
    CONSTRAINT matches_track_a FOREIGN KEY (track_a_id) REFERENCES tracks (id) ON DELETE SET NULL,
    CONSTRAINT matches_track_b FOREIGN KEY (track_b_id) REFERENCES tracks (id) ON DELETE SET NULL,
    CONSTRAINT matches_closed_by FOREIGN KEY (closed_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Each device's vote on a match: the gateway's hash of the device, until
-- an hour after the match closes.
CREATE TABLE match_votes (
    match_id  BIGINT UNSIGNED NOT NULL,
    device    VARCHAR(128) NOT NULL,
    choice    ENUM('a', 'b') NOT NULL,
    voted_at  DATETIME(6) NOT NULL,
    PRIMARY KEY (match_id, device),
    KEY match_votes_voted_at (voted_at),
    CONSTRAINT match_votes_match FOREIGN KEY (match_id) REFERENCES matches (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
