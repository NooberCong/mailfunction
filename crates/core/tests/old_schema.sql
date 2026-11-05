-- The store as Phase 0 left it (user_version 2), with a little mail, for the migration test.
CREATE TABLE account (
    id INTEGER PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    security TEXT NOT NULL,
    auth TEXT NOT NULL
);
CREATE TABLE label (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    UNIQUE (account_id, name)
);
CREATE TABLE folder (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
    path TEXT NOT NULL,
    label_id INTEGER NOT NULL REFERENCES label,
    uid_validity INTEGER NOT NULL DEFAULT 0,
    next_uid INTEGER NOT NULL DEFAULT 0,
    backfill_uid INTEGER NOT NULL DEFAULT 0,
    modseq INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, path)
);
CREATE TABLE message (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
    remote_id TEXT NOT NULL,
    gm_thread_id INTEGER,
    subject TEXT NOT NULL,
    from_name TEXT NOT NULL,
    from_addr TEXT NOT NULL,
    date INTEGER NOT NULL,
    size INTEGER NOT NULL,
    seen INTEGER NOT NULL,
    flagged INTEGER NOT NULL,
    UNIQUE (account_id, remote_id)
);
CREATE INDEX message_unseen ON message (id) WHERE seen = 0;
CREATE TABLE placement (
    folder_id INTEGER NOT NULL REFERENCES folder ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES message ON DELETE CASCADE,
    uid INTEGER,
    PRIMARY KEY (folder_id, message_id)
) WITHOUT ROWID;
CREATE UNIQUE INDEX placement_uid ON placement (folder_id, uid);
CREATE INDEX placement_message ON placement (message_id);
CREATE TABLE message_label (
    label_id INTEGER NOT NULL REFERENCES label ON DELETE CASCADE,
    date INTEGER NOT NULL,
    message_id INTEGER NOT NULL REFERENCES message ON DELETE CASCADE,
    PRIMARY KEY (label_id, date, message_id)
) WITHOUT ROWID;
CREATE INDEX message_label_message ON message_label (message_id);
CREATE TABLE bodies.body (
    message_id INTEGER PRIMARY KEY,
    raw BLOB NOT NULL
);
CREATE TABLE image_sender (
    account_id INTEGER NOT NULL REFERENCES account ON DELETE CASCADE,
    address TEXT NOT NULL,
    PRIMARY KEY (account_id, address)
) WITHOUT ROWID;
CREATE TABLE logo (
    domain TEXT PRIMARY KEY,
    image BLOB,
    checked INTEGER NOT NULL
);
PRAGMA user_version = 2;

INSERT INTO account VALUES (1, 'me@gmail.com', 'imap.gmail.com', 993, 'tls', 'google');
INSERT INTO label VALUES (1, 1, 'Inbox', 'inbox');
INSERT INTO label VALUES (2, 1, 'All Mail', 'all');
INSERT INTO folder VALUES (1, 1, '[Gmail]/All Mail', 2, 7, 10, 1, 100);
INSERT INTO message VALUES (1, 1, 'gm:1', 50, 'First', 'A', 'a@x.example', 100, 500, 1, 0);
INSERT INTO message VALUES (2, 1, 'gm:2', 50, 'Second', 'B', 'b@x.example', 200, 500, 0, 0);
INSERT INTO message VALUES (3, 1, 'gm:3', 51, 'Alone', 'C', 'c@x.example', 150, 500, 1, 0);
INSERT INTO placement VALUES (1, 1, 1), (1, 2, 2), (1, 3, 3);
INSERT INTO message_label VALUES (1, 100, 1), (1, 200, 2), (1, 150, 3), (2, 100, 1), (2, 200, 2), (2, 150, 3);
