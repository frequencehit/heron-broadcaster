-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- Cover images are files of the covers folder (docs/library.md#where-the-images-are):
-- `covers` keeps the hash and the size of each image. heron-api writes the
-- images of these columns to the folder before this migration runs
-- (crates/api/src/migrate.rs).

ALTER TABLE covers
    DROP COLUMN image,
    DROP COLUMN thumbnail;
