-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Cover art kept in Heron, per album with exceptions per track
-- (docs/library.md#cover-art).

-- Each image once, by the hash of the image as found, re-encoded twice.
CREATE TABLE covers (
    hash        BINARY(32) NOT NULL,
    -- Of the image as found.
    width       INT UNSIGNED NOT NULL,
    height      INT UNSIGNED NOT NULL,
    -- 600 px on the longest side, JPEG.
    image       MEDIUMBLOB NOT NULL,
    -- 96 px on the longest side, JPEG.
    thumbnail   BLOB NOT NULL,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (hash)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- The album a track belongs to, for its cover: its MusicBrainz release, else
-- its album artist (or artist) and album, lowercased and trimmed. (A CHAR
-- column is trimmed: its value would depend on the SQL mode otherwise.)
ALTER TABLE tracks
    ADD COLUMN album_key CHAR(32) CHARACTER SET ascii AS (MD5(
        CASE
            WHEN TRIM(musicbrainz_release_id) IS NOT NULL
                THEN CONCAT('mb:', TRIM(musicbrainz_release_id))
            WHEN NULLIF(TRIM(album), '') IS NOT NULL
                THEN CONCAT('tag:',
                            LOWER(TRIM(COALESCE(NULLIF(TRIM(album_artist), ''), artist, ''))),
                            '\n', LOWER(TRIM(album)))
        END)) STORED,
    ADD KEY tracks_album_key (album_key);

CREATE TABLE album_covers (
    album_key   CHAR(32) CHARACTER SET ascii NOT NULL,
    cover_hash  BINARY(32) NOT NULL,
    source      ENUM('hand', 'embedded', 'file', 'cover_art_archive') NOT NULL,
    -- The file's name (file), the release ID (cover_art_archive).
    detail      VARCHAR(255) NULL,
    updated_by  BIGINT UNSIGNED NULL,
    updated_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (album_key),
    KEY album_covers_cover (cover_hash),
    CONSTRAINT album_covers_cover FOREIGN KEY (cover_hash) REFERENCES covers (hash),
    CONSTRAINT album_covers_user FOREIGN KEY (updated_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- A track's own cover, in place of its album's.
CREATE TABLE track_covers (
    track_id    BIGINT UNSIGNED NOT NULL,
    cover_hash  BINARY(32) NOT NULL,
    source      ENUM('hand', 'embedded', 'file', 'cover_art_archive') NOT NULL,
    detail      VARCHAR(255) NULL,
    updated_by  BIGINT UNSIGNED NULL,
    updated_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (track_id),
    KEY track_covers_cover (cover_hash),
    CONSTRAINT track_covers_track FOREIGN KEY (track_id) REFERENCES tracks (id) ON DELETE CASCADE,
    CONSTRAINT track_covers_cover FOREIGN KEY (cover_hash) REFERENCES covers (hash),
    CONSTRAINT track_covers_user FOREIGN KEY (updated_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Albums looked up on the Cover Art Archive: not looked up again by themselves.
CREATE TABLE cover_lookups (
    album_key     CHAR(32) CHARACTER SET ascii NOT NULL,
    status        ENUM('found', 'candidates', 'not_found', 'failed') NOT NULL,
    error         VARCHAR(500) NULL,
    looked_up_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (album_key)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- The results of an unsure search, for the review screen.
CREATE TABLE cover_candidates (
    id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    album_key      CHAR(32) CHARACTER SET ascii NOT NULL,
    release_id     CHAR(36) NOT NULL,
    title          VARCHAR(512) NULL,
    artist         VARCHAR(512) NULL,
    date           VARCHAR(32) NULL,
    country        VARCHAR(8) NULL,
    score          TINYINT UNSIGNED NOT NULL,
    thumbnail_url  VARCHAR(2048) NOT NULL,
    status         ENUM('pending', 'accepted', 'refused') NOT NULL DEFAULT 'pending',
    created_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY cover_candidates_release (album_key, release_id),
    KEY cover_candidates_status (status, album_key)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- The station logo, stored like a cover.
ALTER TABLE stations
    ADD COLUMN logo_hash BINARY(32) NULL,
    ADD CONSTRAINT stations_logo FOREIGN KEY (logo_hash) REFERENCES covers (hash);

ALTER TABLE library_tasks
    MODIFY COLUMN kind ENUM('organize_plan', 'organize_apply', 'undo', 'review_apply',
                            'artist_relations', 'edit_tags', 'musicbrainz_details',
                            'staging_import', 'delete', 'empty_trash', 'render_medley',
                            'preview_medley', 'fetch_covers', 'apply_cover') NOT NULL;
