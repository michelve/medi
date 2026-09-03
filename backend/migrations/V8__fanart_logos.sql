-- V8__fanart_logos.sql — fanart.tv title-logo artwork (Task 93).
--
-- A movie's transparent-PNG wordmark logo from fanart.tv, downloaded and served locally like
-- its poster/backdrop. One nullable column: the path (relative to images_dir()) of the cached
-- logo, or NULL when the movie has no logo / fanart is unconfigured / not yet backfilled.
ALTER TABLE movies ADD COLUMN logo_path TEXT;   -- relative to images_dir(): movies/<id>/logo.png
