-- Optional for older jobs: absence remains explicitly unknown in read models.
CREATE TABLE computation_evidence (
 job_id TEXT PRIMARY KEY REFERENCES publication_contracts(job_id),
 evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json))
) STRICT;
CREATE TRIGGER computation_evidence_identity BEFORE INSERT ON computation_evidence
WHEN json_extract(NEW.evidence_json,'$.format_version') IS NOT 1
 OR json_extract(NEW.evidence_json,'$.compute') IS NOT
 (SELECT json_extract(contract_json,'$.compute') FROM publication_contracts WHERE job_id=NEW.job_id)
 OR NOT EXISTS(SELECT 1 FROM jobs WHERE id=NEW.job_id AND state IN ('PLANNED','WAITING_DEPENDENCIES','QUEUED','STARTING'))
 OR EXISTS(SELECT 1 FROM attempts WHERE job_id=NEW.job_id AND state!='STARTING')
BEGIN SELECT RAISE(ABORT,'invalid computation comparison evidence'); END;
CREATE TRIGGER computation_evidence_update BEFORE UPDATE ON computation_evidence BEGIN SELECT RAISE(ABORT,'immutable computation evidence'); END;
CREATE TRIGGER computation_evidence_delete BEFORE DELETE ON computation_evidence BEGIN SELECT RAISE(ABORT,'retained computation evidence'); END;
CREATE INDEX freshness_attempts ON attempts(job_id,started_at_us,attempt_no);
CREATE INDEX freshness_jobs ON jobs(branch_id,dataset_id);
