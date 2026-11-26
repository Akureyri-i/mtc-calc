-- The statistics receiver's tables (tools/diag/worker.js): the record as sent, its day and
-- format version; nothing about who sent it or when within the day.
CREATE TABLE IF NOT EXISTS shots (id INTEGER PRIMARY KEY, day TEXT NOT NULL, v INTEGER NOT NULL, record TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS runs (run TEXT PRIMARY KEY, day TEXT NOT NULL, v INTEGER NOT NULL, record TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS shots_day ON shots (day);
CREATE INDEX IF NOT EXISTS runs_day ON runs (day);
