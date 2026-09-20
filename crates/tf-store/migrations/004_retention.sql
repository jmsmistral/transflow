ALTER TABLE read_leases ADD COLUMN kind TEXT NOT NULL DEFAULT 'query' CHECK(kind IN ('query','plan','copy','build'));
ALTER TABLE read_leases ADD COLUMN released INTEGER NOT NULL DEFAULT 0 CHECK(released IN (0,1));
CREATE TABLE retention_clock(singleton INTEGER PRIMARY KEY CHECK(singleton=1), now_us INTEGER NOT NULL) STRICT;
INSERT INTO retention_clock VALUES(1,0);
-- A claim excludes new readers/publications until collection finishes or is abandoned.
-- Physical deletion and crash reconciliation are deliberately owned by the later GC service.
CREATE TABLE artifact_gc_claims(
 digest TEXT PRIMARY KEY REFERENCES artifacts(digest),
 id TEXT NOT NULL UNIQUE,
 claimed_at_us INTEGER NOT NULL
) STRICT;
CREATE TRIGGER gc_blocks_head_insert BEFORE INSERT ON dataset_heads
WHEN EXISTS(SELECT 1 FROM dataset_versions v JOIN artifact_gc_claims g ON g.digest=v.artifact_digest WHERE v.id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'artifact collection in progress'); END;
CREATE TRIGGER gc_blocks_head_update BEFORE UPDATE ON dataset_heads
WHEN EXISTS(SELECT 1 FROM dataset_versions v JOIN artifact_gc_claims g ON g.digest=v.artifact_digest WHERE v.id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'artifact collection in progress'); END;
