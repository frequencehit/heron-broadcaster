-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Likes (docs/listener-interaction.md#likes): listeners like or dislike the
-- track playing. Not in the configuration backups: with the history, in the
-- database dump.

-- The votes of each play, kept as long as the history. The play's station,
-- track and start are copied here so that a period's ranking (the week's
-- most liked, the Top 30) adds them up without the history.
CREATE TABLE play_votes (
    play_id     BIGINT UNSIGNED NOT NULL,
    station_id  BIGINT UNSIGNED NOT NULL,
    track_id    BIGINT UNSIGNED NOT NULL,
    started_at  DATETIME(6) NOT NULL,
    likes       INT UNSIGNED NOT NULL DEFAULT 0,
    dislikes    INT UNSIGNED NOT NULL DEFAULT 0,
    PRIMARY KEY (play_id),
    KEY play_votes_station (station_id, started_at),
    KEY play_votes_track (track_id, station_id, started_at),
    CONSTRAINT play_votes_play FOREIGN KEY (play_id) REFERENCES play_history (id) ON DELETE CASCADE,
    CONSTRAINT play_votes_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT play_votes_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Each device's vote on a play, so that a device changes its vote instead
-- of adding one: the gateway's hash of the device, never an address.
-- Deleted an hour after the play ends, a day after the vote at the latest.
CREATE TABLE play_vote_devices (
    play_id     BIGINT UNSIGNED NOT NULL,
    device      VARCHAR(128) CHARACTER SET ascii NOT NULL,
    vote        ENUM('like', 'dislike') NOT NULL,
    voted_at    DATETIME(6) NOT NULL,
    PRIMARY KEY (play_id, device),
    KEY play_vote_devices_voted_at (voted_at),
    CONSTRAINT play_vote_devices_play FOREIGN KEY (play_id) REFERENCES play_history (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
