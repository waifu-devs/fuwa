-- How much a server's recordings may come to, all together (unset: the
-- instance default, FUWA_LIMIT_RECORDING_STORAGE, unlimited unless set).
ALTER TABLE limits ADD COLUMN recording_bytes INTEGER;
