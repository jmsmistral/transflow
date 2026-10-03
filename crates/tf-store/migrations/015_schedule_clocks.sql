-- Durable UTC clock high-water keys and bounded pending intended ticks.
CREATE TABLE schedule_clock_state (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules(id),
    trigger_epoch TEXT NOT NULL,
    cursor_at_us INTEGER NOT NULL CHECK(cursor_at_us>=0),
    cursor_leaf INTEGER NOT NULL CHECK(cursor_leaf BETWEEN 0 AND 63),
    matched_count INTEGER NOT NULL CHECK(matched_count>=0),
    missed_count INTEGER NOT NULL CHECK(missed_count>=0),
    ignored_count INTEGER NOT NULL CHECK(ignored_count>=0),
    coalesced_count INTEGER NOT NULL CHECK(coalesced_count>=0)
) STRICT;
CREATE TABLE schedule_clock_ticks (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL REFERENCES schedules(id),
    trigger_epoch TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    leaf_order INTEGER NOT NULL CHECK(leaf_order BETWEEN 0 AND 63),
    at_us INTEGER NOT NULL CHECK(at_us>=0),
    tzdb_version TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK(disposition IN ('PENDING','DELIVERED','COALESCED','EXPIRED')),
    UNIQUE(schedule_id,trigger_epoch,leaf_id,at_us)
) STRICT;
CREATE INDEX schedule_pending_ticks ON schedule_clock_ticks(schedule_id,trigger_epoch,at_us,leaf_order)
WHERE disposition='PENDING';
CREATE TRIGGER schedule_tick_identity_immutable BEFORE UPDATE ON schedule_clock_ticks
WHEN NEW.id!=OLD.id OR NEW.schedule_id!=OLD.schedule_id OR NEW.trigger_epoch!=OLD.trigger_epoch
 OR NEW.leaf_id!=OLD.leaf_id OR NEW.leaf_order!=OLD.leaf_order OR NEW.at_us!=OLD.at_us OR NEW.tzdb_version!=OLD.tzdb_version
BEGIN SELECT RAISE(ABORT,'intended tick identity is immutable'); END;
