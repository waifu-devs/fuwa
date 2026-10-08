-- A ceiling on cameras in the server's voice channels: the tallest picture,
-- in pixels, and the most frames a second. NULL for none.
ALTER TABLE server ADD COLUMN camera_max_height INTEGER;
ALTER TABLE server ADD COLUMN camera_max_fps INTEGER;
