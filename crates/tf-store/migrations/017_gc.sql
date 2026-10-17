-- Durable observation and deletion intents. Unknown objects are never aged by mtime.
CREATE TABLE gc_items (
 kind TEXT NOT NULL CHECK(kind IN ('artifact','source','logs')),
 object_key TEXT NOT NULL,
 first_seen_us INTEGER NOT NULL CHECK(first_seen_us>=0),
 state TEXT NOT NULL CHECK(state IN ('OBSERVED','CLAIMED','DELETED')),
 claim_id TEXT UNIQUE,
 claimed_at_us INTEGER,
 deleted_at_us INTEGER,
 PRIMARY KEY(kind,object_key),
 CHECK((state='OBSERVED' AND claim_id IS NULL) OR (state IN ('CLAIMED','DELETED') AND claim_id IS NOT NULL))
) STRICT;
CREATE INDEX gc_pending ON gc_items(state,kind,object_key);
-- Missing/expired payloads retain immutable version/provenance metadata.
CREATE TRIGGER gc_blocks_source_pin BEFORE INSERT ON pins
WHEN NEW.source_snapshot_id IS NOT NULL AND EXISTS(SELECT 1 FROM gc_items g WHERE g.kind='source' AND g.object_key=NEW.source_snapshot_id AND g.state IN ('CLAIMED','DELETED'))
BEGIN SELECT RAISE(ABORT,'source data expired or collection in progress'); END;
CREATE TRIGGER gc_blocks_expired_head_insert BEFORE INSERT ON dataset_heads
WHEN EXISTS(SELECT 1 FROM dataset_versions v JOIN gc_items g ON g.kind='artifact' AND g.object_key=v.artifact_digest WHERE v.id=NEW.version_id AND g.state='DELETED')
BEGIN SELECT RAISE(ABORT,'data expired'); END;
CREATE TRIGGER gc_blocks_expired_head_update BEFORE UPDATE ON dataset_heads
WHEN EXISTS(SELECT 1 FROM dataset_versions v JOIN gc_items g ON g.kind='artifact' AND g.object_key=v.artifact_digest WHERE v.id=NEW.version_id AND g.state='DELETED')
BEGIN SELECT RAISE(ABORT,'data expired'); END;
CREATE TRIGGER gc_blocks_expired_source_plan_insert BEFORE INSERT ON build_plans
WHEN EXISTS(SELECT 1 FROM gc_items g WHERE g.kind='source' AND g.object_key=NEW.source_snapshot_id AND g.state IN ('CLAIMED','DELETED'))
BEGIN SELECT RAISE(ABORT,'source data expired'); END;
CREATE TRIGGER gc_blocks_expired_source_plan_update BEFORE UPDATE ON build_plans
WHEN EXISTS(SELECT 1 FROM gc_items g WHERE g.kind='source' AND g.object_key=NEW.source_snapshot_id AND g.state IN ('CLAIMED','DELETED'))
BEGIN SELECT RAISE(ABORT,'source data expired'); END;
CREATE TABLE gc_deletions(
 claim_id TEXT PRIMARY KEY,
 kind TEXT NOT NULL CHECK(kind IN ('artifact','source','logs')),
 object_key TEXT NOT NULL,
 deleted_at_us INTEGER NOT NULL
) STRICT;
CREATE TRIGGER gc_deletions_immutable BEFORE UPDATE ON gc_deletions BEGIN SELECT RAISE(ABORT,'immutable GC evidence'); END;
CREATE TRIGGER gc_deletions_append_only BEFORE DELETE ON gc_deletions BEGIN SELECT RAISE(ABORT,'immutable GC evidence'); END;
