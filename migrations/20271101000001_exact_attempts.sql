-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- The attempts page (docs/rotation.md#the-attempts-page): each time the
-- queue manager built fills before an exact time, a calculation with its
-- attempts (every fill built, as JSON), and a light row (no attempts) each
-- time a pick followed the fill kept. An attempt chosen by hand is kept
-- with its calculation. Deleted after 7 days.

CREATE TABLE exact_calculations (
    id              BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    station_id      BIGINT UNSIGNED NOT NULL,
    -- The exact time, its window (Unix ms), mode, clock and slot.
    exact_at_ms     BIGINT NOT NULL,
    window_from_ms  BIGINT NOT NULL,
    window_cut_ms   BIGINT NOT NULL,
    mode            ENUM('hard', 'soft') NOT NULL,
    clock_id        BIGINT UNSIGNED NULL,
    exact_slot      INT UNSIGNED NOT NULL,
    -- Where the fills start: the end of the queue (Unix ms).
    from_ms         BIGINT NOT NULL,
    computed_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    -- Why: the first pick for this exact time, a restart, a change of the
    -- queue, a fill chosen by hand that no longer held, back to automatic;
    -- or a pick that followed the fill kept.
    reason          ENUM('first', 'restart', 'queue_changed', 'hand_dropped', 'automatic',
                         'followed') NOT NULL,
    -- The calculation whose fill a followed pick follows (it may be
    -- deleted first: no foreign key).
    follows_id      BIGINT UNSIGNED NULL,
    fills           SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    batches         SMALLINT UNSIGNED NOT NULL DEFAULT 0,
    stop            ENUM('followed', 'landed', 'max_fills', 'time') NOT NULL,
    duration_ms     INT UNSIGNED NOT NULL,
    -- The attempt kept, by its number, and how the fill kept ends.
    kept_attempt    SMALLINT UNSIGNED NULL,
    offset_ms       BIGINT NULL,
    in_window       BOOLEAN NOT NULL,
    faded           BOOLEAN NOT NULL,
    filling_ms      INT UNSIGNED NOT NULL,
    relaxed_steps   INT UNSIGNED NOT NULL,
    age_ms          BIGINT NOT NULL DEFAULT 0,
    -- The fill followed was chosen by hand.
    by_hand         BOOLEAN NOT NULL DEFAULT FALSE,
    -- An attempt chosen by hand on the page: which, by whom, when.
    chosen_attempt  SMALLINT UNSIGNED NULL,
    chosen_by       BIGINT UNSIGNED NULL,
    chosen_at       DATETIME(6) NULL,
    -- Every fill built (heron_rotation::Attempt); NULL for a followed pick.
    attempts        JSON NULL,
    PRIMARY KEY (id),
    KEY exact_calculations_exact (station_id, exact_at_ms),
    KEY exact_calculations_computed (computed_at),
    KEY exact_calculations_follows (follows_id),
    CONSTRAINT exact_calculations_station FOREIGN KEY (station_id) REFERENCES stations (id) ON DELETE CASCADE,
    CONSTRAINT exact_calculations_user FOREIGN KEY (chosen_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
