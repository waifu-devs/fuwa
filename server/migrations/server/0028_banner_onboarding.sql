-- Server banners (with the point to keep in view and a color to go with
-- them) and onboarding, the steps new members go through once they're in.

ALTER TABLE server ADD COLUMN banner_url TEXT NOT NULL DEFAULT '';
ALTER TABLE server ADD COLUMN banner_focus_x INTEGER NOT NULL DEFAULT 50;
ALTER TABLE server ADD COLUMN banner_focus_y INTEGER NOT NULL DEFAULT 50;
-- 0xRRGGBB, or NULL for none.
ALTER TABLE server ADD COLUMN accent_color INTEGER;
-- A fuwa.v1.Onboarding, as protobuf. Empty for none.
ALTER TABLE server ADD COLUMN onboarding BLOB NOT NULL DEFAULT x'';

-- When each member last went through the onboarding, or NULL.
ALTER TABLE members ADD COLUMN onboarded_at INTEGER;
