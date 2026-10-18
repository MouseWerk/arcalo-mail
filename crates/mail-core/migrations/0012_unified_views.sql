-- „Alle Postfächer“: the folders of a role in every account, newest first, a page at a time by
-- (date, id) without sorting the whole list (`unified::folder_page`).
CREATE INDEX messages_by_date ON messages (COALESCE(date, internal_date) DESC, id DESC) WHERE hidden = 0;

-- Unread and visible counts per folder read from the index alone (the folder list, the account
-- switcher's numbers, the app's badge).
CREATE INDEX messages_unread ON messages (mailbox_id, hidden, seen);

-- Messages back from snoozing sort by when they came back: a page of the sorted inbox takes
-- them along without reading the whole inbox.
CREATE INDEX messages_bumped ON messages (bumped_at) WHERE bumped_at IS NOT NULL;

-- The inbox tabs' numbers from the index alone, the few senders the user sorted (or wrote to)
-- counted again on their own (`triage::counts`).
CREATE INDEX messages_tabs ON messages (mailbox_id, hidden, category, seen, snoozed_until);
