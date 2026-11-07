-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Stream servers per station (docs/outputs.md#stream-servers): the address,
-- port and credentials outputs going to one server share, and relay servers
-- repeating a master's mounts, read for the listener statistics
-- (docs/statistics.md#relay-servers). An output picks a server and keeps
-- its mount in `config`, and the station details it overrides in `details`.
-- The next migration groups the existing outputs into servers.
-- Configuration: in backups.

CREATE TABLE stream_servers (
    id                  BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id          BIGINT UNSIGNED NOT NULL,
    name                VARCHAR(255) NOT NULL,
    kind                ENUM('icecast') NOT NULL DEFAULT 'icecast',
    host                VARCHAR(255) NOT NULL,
    port                SMALLINT UNSIGNED NOT NULL DEFAULT 8000,
    tls                 BOOLEAN NOT NULL DEFAULT FALSE,
    -- NULL: `source`.
    source_username     VARCHAR(255) NULL,
    source_password     VARCHAR(255) NULL,
    -- For the listener statistics, never pushed to the engine. NULL: `admin`.
    admin_username      VARCHAR(255) NULL,
    admin_password      VARCHAR(255) NULL,
    -- As listeners reach the server; NULL: http://host:port.
    public_url          VARCHAR(2048) NULL,
    -- A relay of this master: no output sends to it, it is only read.
    relay_of            BIGINT UNSIGNED NULL,
    -- The mounts it relays, `[{"mount": "/live", "relay_mount": "/r"}]`;
    -- NULL: all of the master's, under the same names.
    relay_mounts        JSON NULL,
    -- Its connection to the master counts as 1 listener per mount.
    relay_subtract_one  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY stream_servers_name (station_id, name),
    KEY stream_servers_relay_of (relay_of),
    CONSTRAINT stream_servers_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    -- The API refuses to delete a master with relays; a restore may.
    CONSTRAINT stream_servers_relay FOREIGN KEY (relay_of) REFERENCES stream_servers (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- `statistics` stays, NULL: older backups bring their admin credentials
-- through it, for the next migration to move onto the servers.
ALTER TABLE outputs
    ADD COLUMN server_id BIGINT UNSIGNED NULL AFTER kind,
    ADD COLUMN details JSON NULL AFTER config,
    ADD KEY outputs_server (server_id),
    ADD CONSTRAINT outputs_server FOREIGN KEY (server_id) REFERENCES stream_servers (id) ON DELETE RESTRICT;

-- Samples and sessions per server read: the master, or a relay (0: before
-- servers, set by the next migration). Not a foreign key: they stay when
-- a relay is deleted.
ALTER TABLE listener_samples
    ADD COLUMN server_id BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER output_id,
    DROP PRIMARY KEY,
    ADD PRIMARY KEY (output_id, server_id, sampled_at);

ALTER TABLE listener_sessions
    ADD COLUMN server_id BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER output_id;
