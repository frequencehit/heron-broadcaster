-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Certification deliveries (docs/certification.md#automatic-delivery): a
-- day's export sent by FTPS to a certification body at a set time, the
-- days sent and each of their attempts. The passwords are never sent back
-- by the API.

CREATE TABLE certification_deliveries (
    id            BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id    BIGINT UNSIGNED NOT NULL,
    name          VARCHAR(255) NOT NULL,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    server_id     BIGINT UNSIGNED NOT NULL,
    -- Every mount when NULL.
    mount         VARCHAR(255) NULL,
    -- SFTP comes next.
    protocol      ENUM('ftps') NOT NULL,
    host          VARCHAR(255) NOT NULL,
    port          SMALLINT UNSIGNED NOT NULL,
    username      VARCHAR(255) NOT NULL,
    password      VARCHAR(255) NULL,
    remote_dir    VARCHAR(1024) NOT NULL DEFAULT '',
    file_name     VARCHAR(255) NOT NULL,
    format        VARCHAR(16) NOT NULL,
    compression   ENUM('none', 'gzip') NOT NULL DEFAULT 'none',
    -- Minutes after midnight, in the station's time zone.
    send_at_min   SMALLINT UNSIGNED NOT NULL,
    -- The day sent: 1 the day before.
    days_back     TINYINT UNSIGNED NOT NULL DEFAULT 1,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY certification_deliveries_station (station_id),
    CONSTRAINT certification_deliveries_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT certification_deliveries_server FOREIGN KEY (server_id) REFERENCES stream_servers (id) ON DELETE CASCADE
);

-- A day sent, at its time or by "Send now"; the last 30 are kept per
-- delivery.
CREATE TABLE certification_delivery_runs (
    id              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    delivery_id     BIGINT UNSIGNED NOT NULL,
    day             DATE NOT NULL,
    -- The day, for a run started at its time: one per day. NULL for "Send now".
    scheduled_day   DATE NULL,
    state           ENUM('pending', 'sending', 'delivered', 'failed') NOT NULL DEFAULT 'pending',
    attempts        SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    next_attempt_at DATETIME(6) NULL,
    -- A run being sent is taken again after this, if the API stopped.
    claimed_until   DATETIME(6) NULL,
    file_name       VARCHAR(255) NULL,
    bytes           BIGINT UNSIGNED NULL,
    -- The day's gaps, ISO 8601 intervals in UTC, comma-separated.
    gaps            TEXT NULL,
    error           TEXT NULL,
    created_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    finished_at     DATETIME(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY certification_delivery_runs_scheduled (delivery_id, scheduled_day),
    KEY certification_delivery_runs_due (state, next_attempt_at),
    CONSTRAINT certification_delivery_runs_delivery FOREIGN KEY (delivery_id) REFERENCES certification_deliveries (id) ON DELETE CASCADE
);

CREATE TABLE certification_delivery_attempts (
    id            BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    run_id        BIGINT UNSIGNED NOT NULL,
    attempted_at  DATETIME(6) NOT NULL,
    ok            BOOLEAN NOT NULL,
    error         TEXT NULL,
    file_name     VARCHAR(255) NULL,
    bytes         BIGINT UNSIGNED NULL,
    PRIMARY KEY (id),
    KEY certification_delivery_attempts_run (run_id),
    CONSTRAINT certification_delivery_attempts_run FOREIGN KEY (run_id) REFERENCES certification_delivery_runs (id) ON DELETE CASCADE
);
