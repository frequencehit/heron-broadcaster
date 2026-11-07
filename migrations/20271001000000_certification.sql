-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Audience certification (docs/certification.md): the stream servers'
-- access logs, where Heron reads them and how far it has, each line kept
-- unchanged with what was parsed from it, the hours a log could not be
-- read, and who downloaded what. The lines hold listeners' addresses:
-- deleted after the retention (settings `certification`).

CREATE TABLE access_logs (
    server_id     BIGINT UNSIGNED NOT NULL,
    station_id    BIGINT UNSIGNED NOT NULL,
    source        ENUM('file', 'url') NOT NULL,
    -- Under the access logs folder, for a file.
    path          VARCHAR(1024) NULL,
    url           VARCHAR(2048) NULL,
    username      VARCHAR(255) NULL,
    password      VARCHAR(255) NULL,
    -- Bytes of the current file read, up to the end of its last full line.
    position      BIGINT UNSIGNED NOT NULL DEFAULT 0,
    read_at       DATETIME(6) NULL,
    -- The last reading that worked: a gap starts there.
    read_ok_at    DATETIME(6) NULL,
    error         TEXT NULL,
    error_since   DATETIME(6) NULL,
    PRIMARY KEY (server_id),
    KEY access_logs_station (station_id),
    CONSTRAINT access_logs_server FOREIGN KEY (server_id) REFERENCES stream_servers (id) ON DELETE CASCADE,
    CONSTRAINT access_logs_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
);

CREATE TABLE access_log_lines (
    id            BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id    BIGINT UNSIGNED NOT NULL,
    server_id     BIGINT UNSIGNED NOT NULL,
    -- The line's date: the end of the session.
    logged_at     DATETIME(6) NOT NULL,
    line          TEXT NOT NULL,
    -- A listener's session, with what follows; the other lines are kept
    -- for the raw export.
    session       BOOLEAN NOT NULL,
    address       VARCHAR(64) NULL,
    mount         VARCHAR(255) NULL,
    status        SMALLINT UNSIGNED NULL,
    bytes         BIGINT UNSIGNED NULL,
    referer       TEXT NULL,
    user_agent    TEXT NULL,
    duration_s    INT UNSIGNED NULL,
    PRIMARY KEY (id),
    KEY access_log_lines_server_day (server_id, logged_at),
    KEY access_log_lines_logged (logged_at),
    CONSTRAINT access_log_lines_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
);

-- The hours a log could not be read, or whose lines were lost to a
-- rotation without its previous file.
CREATE TABLE access_log_gaps (
    id            BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id    BIGINT UNSIGNED NOT NULL,
    server_id     BIGINT UNSIGNED NOT NULL,
    from_at       DATETIME(6) NOT NULL,
    to_at         DATETIME(6) NOT NULL,
    reason        VARCHAR(255) NOT NULL,
    PRIMARY KEY (id),
    KEY access_log_gaps_server (server_id, from_at),
    CONSTRAINT access_log_gaps_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
);

CREATE TABLE certification_downloads (
    id            BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id    BIGINT UNSIGNED NOT NULL,
    user_id       BIGINT UNSIGNED NULL,
    username      VARCHAR(255) NOT NULL,
    server_id     BIGINT UNSIGNED NOT NULL,
    day           DATE NOT NULL,
    mount         VARCHAR(255) NULL,
    format        VARCHAR(16) NOT NULL,
    downloaded_at DATETIME(6) NOT NULL,
    PRIMARY KEY (id),
    KEY certification_downloads_station (station_id, downloaded_at),
    CONSTRAINT certification_downloads_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE
);
