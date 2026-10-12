-- Mod View (ModerationService, docs/mod-view.md).
--
-- A case: a message AutoMod caught, or a message or member someone
-- reported. While a report case is open, more reports of the same message
-- (or member) join it: `open_key` is 'm:<message id>' or 'u:<user id>' for
-- open report cases and NULL otherwise, unique, so two reports at once
-- clash and the second joins the first. `content` is what the message said
-- when the case opened; `auto_mod` the AutoModAlert (protobuf) for AutoMod
-- cases. Deleting the person's account deletes the cases about them.
CREATE TABLE mod_cases (
    id TEXT NOT NULL PRIMARY KEY,
    kind INTEGER NOT NULL,
    status INTEGER NOT NULL,
    subject_id TEXT NOT NULL,
    channel_id TEXT NOT NULL DEFAULT '',
    message_id TEXT NOT NULL DEFAULT '',
    thread_id TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    auto_mod BLOB,
    open_key TEXT,
    report_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    closed_by TEXT NOT NULL DEFAULT '',
    closed_at INTEGER,
    resolution TEXT NOT NULL DEFAULT ''
);

CREATE UNIQUE INDEX mod_cases_open ON mod_cases (open_key);
CREATE INDEX mod_cases_by_subject ON mod_cases (subject_id, id);
CREATE INDEX mod_cases_by_status ON mod_cases (status, id);

-- One row per person and case. Deleting a reporter's account deletes their
-- reports (and a report case nobody's report is left in).
CREATE TABLE mod_reports (
    case_id TEXT NOT NULL,
    reporter_id TEXT NOT NULL,
    reason INTEGER NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (case_id, reporter_id)
);

CREATE INDEX mod_reports_by_reporter ON mod_reports (reporter_id);

-- Moderators' notes about a person.
CREATE TABLE mod_notes (
    id TEXT NOT NULL PRIMARY KEY,
    subject_id TEXT NOT NULL,
    author_id TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX mod_notes_by_subject ON mod_notes (subject_id, id);

-- Audit entries about a person, for their history in Mod View.
CREATE INDEX audit_by_target ON audit (target_id, id);

-- MODERATE (bit 29) goes to the roles that could already time out, kick or
-- ban (bits 9, 7, 8): the moderators. Nothing could have named this bit
-- before, so nobody loses a choice they made.
UPDATE roles SET permissions = permissions | 536870912 WHERE permissions & 896 != 0;
