-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- What each background service says of itself, every half minute: its
-- version, since when it runs, the machine it runs on and its configuration
-- (the values that are not secret). Read by the About page. Not in backups:
-- it describes the running processes, not the station. See
-- docs/roadmap/m17-about-page.md.

CREATE TABLE service_heartbeats (
    service    VARCHAR(32)  NOT NULL,
    instance   VARCHAR(128) NOT NULL,
    version    VARCHAR(64)  NOT NULL,
    revision   VARCHAR(64)  NULL,
    build_date VARCHAR(64)  NULL,
    started_at DATETIME(6)  NOT NULL,
    seen_at    DATETIME(6)  NOT NULL,
    os         VARCHAR(255) NULL,
    kernel     VARCHAR(255) NULL,
    arch       VARCHAR(64)  NULL,
    container  BOOLEAN      NOT NULL DEFAULT FALSE,
    config     JSON         NOT NULL,
    PRIMARY KEY (service, instance)
) ENGINE = InnoDB;
