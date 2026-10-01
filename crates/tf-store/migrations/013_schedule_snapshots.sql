-- Replace the pre-release definition-history placeholders. Accepted execution
-- evidence is copied before old definitions are removed; active legacy definitions
-- require an explicit normalized save before future scheduling can dispatch them.
PRAGMA defer_foreign_keys=ON;
CREATE TABLE schedules_next (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name TEXT NOT NULL UNIQUE,
    definition_json TEXT NOT NULL CHECK(json_valid(definition_json) AND length(definition_json)<=262144),
    etag TEXT NOT NULL,
    trigger_epoch TEXT NOT NULL,
    paused INTEGER NOT NULL CHECK(paused IN (0,1)),
    needs_review INTEGER NOT NULL CHECK(needs_review IN (0,1)),
    saved_at_us INTEGER NOT NULL,
    saved_by TEXT NOT NULL,
    event_cursor INTEGER NOT NULL CHECK(event_cursor>=0),
    deleted_at_us INTEGER
) STRICT;
CREATE TABLE schedule_occurrences_next (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL REFERENCES schedules_next(id),
    trigger_epoch TEXT NOT NULL,
    evidence_digest TEXT NOT NULL,
    logical_fire_at_us INTEGER NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    execution_json TEXT NOT NULL CHECK(json_valid(execution_json)),
    disposition TEXT NOT NULL,
    build_id TEXT REFERENCES builds(id) DEFERRABLE INITIALLY DEFERRED,
    UNIQUE(schedule_id,trigger_epoch,evidence_digest)
) STRICT;
CREATE TABLE trigger_tokens_next (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL REFERENCES schedules_next(id),
    trigger_epoch TEXT NOT NULL,
    leaf_id TEXT NOT NULL,
    event_id TEXT REFERENCES events(id),
    tick_id TEXT,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    seen_at_us INTEGER NOT NULL,
    expires_at_us INTEGER,
    consumed_by TEXT REFERENCES schedule_occurrences_next(id),
    CHECK((event_id IS NULL)!=(tick_id IS NULL))
) STRICT;
INSERT INTO schedules_next
SELECT s.id,(SELECT id FROM workspaces LIMIT 1),s.name,
 json_object('format_version',1,'name',s.name,'description','','build',json(COALESCE(r.build_template_json,'{}')),'trigger',json(COALESCE(r.trigger_json,'{}')),'policies',json(COALESCE(r.policies_json,'{}'))),
 lower(hex(randomblob(32))), 'legacy-'||COALESCE(s.active_revision,0),s.paused,1,COALESCE(r.created_at_us,0),COALESCE(r.author,'migration'),
 COALESCE((SELECT max(sequence) FROM events),0),s.deleted_at_us
FROM schedules s LEFT JOIN schedule_revisions r ON r.schedule_id=s.id AND r.revision=s.active_revision;
INSERT INTO schedule_occurrences_next
SELECT o.id,o.schedule_id,'legacy-'||o.revision,o.evidence_digest,o.logical_fire_at_us,o.payload_json,
 json_object('build',json(r.build_template_json),'policies',json(r.policies_json)),o.disposition,o.build_id
FROM schedule_occurrences o JOIN schedule_revisions r ON r.schedule_id=o.schedule_id AND r.revision=o.revision;
INSERT INTO trigger_tokens_next
SELECT id,schedule_id,'legacy-'||revision,leaf_id,event_id,tick_id,payload_json,seen_at_us,expires_at_us,consumed_by FROM trigger_tokens;
DROP TABLE trigger_tokens;
DROP TABLE schedule_occurrences;
DROP TABLE schedules;
DROP TABLE schedule_revisions;
ALTER TABLE schedules_next RENAME TO schedules;
ALTER TABLE schedule_occurrences_next RENAME TO schedule_occurrences;
ALTER TABLE trigger_tokens_next RENAME TO trigger_tokens;
CREATE TRIGGER schedule_execution_immutable BEFORE UPDATE ON schedule_occurrences
WHEN NEW.id!=OLD.id OR NEW.schedule_id!=OLD.schedule_id OR NEW.trigger_epoch!=OLD.trigger_epoch OR NEW.evidence_digest!=OLD.evidence_digest OR NEW.execution_json!=OLD.execution_json OR NEW.payload_json!=OLD.payload_json OR NEW.logical_fire_at_us!=OLD.logical_fire_at_us
BEGIN SELECT RAISE(ABORT,'immutable accepted schedule evidence'); END;
