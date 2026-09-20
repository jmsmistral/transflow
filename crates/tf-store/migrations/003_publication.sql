-- Freeze accepted output generations and complete input/check contracts before execution.
ALTER TABLE write_reservations ADD COLUMN expected_head_generation INTEGER NOT NULL DEFAULT 0 CHECK(expected_head_generation>=0);
CREATE TABLE publication_contracts (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id),
    contract_json TEXT NOT NULL CHECK(json_valid(contract_json))
) STRICT;
CREATE TRIGGER immutable_publication_contracts_update BEFORE UPDATE ON publication_contracts BEGIN SELECT RAISE(ABORT,'immutable publication contract'); END;
CREATE TRIGGER immutable_publication_contracts_delete BEFORE DELETE ON publication_contracts BEGIN SELECT RAISE(ABORT,'immutable publication contract'); END;
CREATE TABLE version_check_results (
    version_id TEXT NOT NULL REFERENCES dataset_versions(id),
    result_id TEXT NOT NULL REFERENCES check_results(id),
    PRIMARY KEY(version_id,result_id)
) STRICT;
CREATE TRIGGER version_check_attempt BEFORE INSERT ON version_check_results
WHEN NOT EXISTS(SELECT 1 FROM dataset_versions v JOIN check_results r ON r.attempt_id=v.attempt_id WHERE v.id=NEW.version_id AND r.id=NEW.result_id)
BEGIN SELECT RAISE(ABORT,'check result belongs to another attempt'); END;
CREATE TRIGGER immutable_version_checks_update BEFORE UPDATE ON version_check_results BEGIN SELECT RAISE(ABORT,'immutable version checks'); END;
CREATE TRIGGER immutable_version_checks_delete BEFORE DELETE ON version_check_results BEGIN SELECT RAISE(ABORT,'immutable version checks'); END;
CREATE TRIGGER published_check_update BEFORE UPDATE ON check_results WHEN EXISTS(SELECT 1 FROM version_check_results WHERE result_id=OLD.id) BEGIN SELECT RAISE(ABORT,'published check is immutable'); END;
CREATE TRIGGER published_check_delete BEFORE DELETE ON check_results WHEN EXISTS(SELECT 1 FROM version_check_results WHERE result_id=OLD.id) BEGIN SELECT RAISE(ABORT,'published check is immutable'); END;
CREATE UNIQUE INDEX one_version_per_attempt ON dataset_versions(attempt_id) WHERE attempt_id IS NOT NULL;
CREATE TRIGGER publication_intent_identity BEFORE UPDATE ON publication_intents
WHEN NEW.attempt_id!=OLD.attempt_id OR NEW.planned_version_id!=OLD.planned_version_id
OR NEW.artifact_digest!=OLD.artifact_digest OR NEW.expected_head_generation!=OLD.expected_head_generation
OR NEW.session_id!=OLD.session_id OR NEW.fence!=OLD.fence
OR OLD.state!='PREPARED' OR NEW.state NOT IN ('COMMITTED','ABANDONED')
BEGIN SELECT RAISE(ABORT,'immutable publication intent identity or terminal state'); END;
CREATE TRIGGER publication_intent_delete BEFORE DELETE ON publication_intents BEGIN SELECT RAISE(ABORT,'retained publication intent'); END;
