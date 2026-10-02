-- Keep automatic evaluation off consumed history. Original time precedes arrival
-- order; expiry is checked against the caller's frozen clock inside writer ownership.
CREATE INDEX schedule_pending_tokens ON trigger_tokens(
    schedule_id, trigger_epoch, leaf_id,
    CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER) DESC,
    CAST(json_extract(payload_json,'$.event_sequence') AS INTEGER) DESC, id DESC
) WHERE consumed_by IS NULL;
CREATE INDEX schedule_pending_occurrences ON schedule_occurrences(schedule_id)
WHERE disposition IN ('ACCEPTED','QUEUED','HELD');
