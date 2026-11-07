-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Dedications (docs/listener-interaction.md#dedications): listeners' requests
-- for a track with a message, sent through the gateway during the show's
-- hours, accepted into the queue or refused by an operator. Deleted after
-- the station's retention (30 days by default); the device's hash is
-- cleared when the opening range closes. No IP address.
-- In the database dump of backups; the settings are in the configuration.

CREATE TABLE dedications (
    id                 BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id         BIGINT UNSIGNED NOT NULL,
    -- The random key of the follow link.
    follow_key         CHAR(32) NOT NULL,
    track_id           BIGINT UNSIGNED NULL,
    from_name          VARCHAR(40) NOT NULL,
    to_name            VARCHAR(60) NOT NULL,
    message            VARCHAR(500) NOT NULL,
    -- The gateway's hash of the device, until the range closes.
    device             VARCHAR(128) NULL,
    status             ENUM('pending', 'accepted', 'played', 'refused') NOT NULL DEFAULT 'pending',
    reason             ENUM('just_played', 'unavailable', 'inappropriate', 'closed', 'removed') NULL,
    submitted_at       DATETIME(6) NOT NULL,
    -- The opening range it was sent in.
    opens_at           DATETIME(6) NOT NULL,
    closes_at          DATETIME(6) NOT NULL,
    decided_at         DATETIME(6) NULL,
    decided_by         BIGINT UNSIGNED NULL,
    planned_at         DATETIME(6) NULL,
    -- Its queue entry, for the engine started then.
    engine_started_at  BIGINT UNSIGNED NULL,
    entry_id           BIGINT UNSIGNED NULL,
    play_id            BIGINT UNSIGNED NULL,
    played_at          DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY dedications_follow_key (follow_key),
    KEY dedications_range (station_id, opens_at),
    KEY dedications_entry (station_id, entry_id),
    KEY dedications_device (station_id, device),
    KEY dedications_submitted_at (submitted_at),
    CONSTRAINT dedications_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT dedications_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE SET NULL,
    CONSTRAINT dedications_decided_by FOREIGN KEY (decided_by) REFERENCES users (id) ON DELETE SET NULL,
    CONSTRAINT dedications_play FOREIGN KEY (play_id) REFERENCES play_history (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
