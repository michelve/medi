-- V9__fanart_wallpapers.sql — fanart.tv background wallpaper artwork (Task 95).
--
-- A movie's fanart.tv 1920x1080 background wallpaper (`moviebackground`), downloaded and
-- served locally like its poster/backdrop/logo. Shown on the detail hero in place of the TMDB
-- backdrop when present. One nullable column: the path (relative to images_dir()) of the
-- cached wallpaper, or NULL when the movie has no wallpaper / fanart is unconfigured / not yet
-- backfilled. Fetched from the same /v3/movies/{id} response as the logo (Task 93), so it adds
-- no extra fanart request.
ALTER TABLE movies ADD COLUMN wallpaper_path TEXT;   -- relative to images_dir(): movies/<id>/wallpaper.jpg
