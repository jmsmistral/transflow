-- Reuse retains the original publication and checks; it creates no attempt/version.
CREATE TABLE cached_jobs (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id),
    version_id TEXT NOT NULL REFERENCES dataset_versions(id),
    original_attempt_id TEXT NOT NULL REFERENCES attempts(id),
    generation INTEGER NOT NULL CHECK(generation>0),
    event_id TEXT UNIQUE REFERENCES events(id),
    reused_at_us INTEGER NOT NULL,
    expected_generation INTEGER NOT NULL CHECK(expected_generation>=0),
    session_id TEXT NOT NULL,
    fence INTEGER NOT NULL CHECK(fence>=0)
) STRICT;
CREATE TRIGGER cached_job_identity BEFORE INSERT ON cached_jobs
WHEN NOT EXISTS(SELECT 1 FROM jobs j JOIN dataset_versions v ON v.dataset_id=j.dataset_id
 JOIN attempts a ON a.id=v.attempt_id JOIN jobs original ON original.id=a.job_id
 WHERE j.id=NEW.job_id AND j.state='CACHED' AND v.id=NEW.version_id
 AND a.id=NEW.original_attempt_id AND a.state='SUCCEEDED' AND original.branch_id=j.branch_id)
 OR EXISTS(SELECT 1 FROM attempts WHERE job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'invalid cache identity'); END;
CREATE TRIGGER cached_jobs_immutable BEFORE UPDATE ON cached_jobs BEGIN SELECT RAISE(ABORT,'immutable cache evidence'); END;
CREATE TRIGGER cached_jobs_append_only BEFORE DELETE ON cached_jobs BEGIN SELECT RAISE(ABORT,'retained cache evidence'); END;
CREATE TRIGGER validation_reuse_immutable BEFORE UPDATE ON validation_reuse BEGIN SELECT RAISE(ABORT,'immutable validation reuse'); END;
CREATE TRIGGER validation_reuse_append_only BEFORE DELETE ON validation_reuse BEGIN SELECT RAISE(ABORT,'retained validation reuse'); END;
CREATE UNIQUE INDEX one_reused_check_per_job ON validation_reuse(job_id,original_result_id);
CREATE INDEX versions_compute_cache ON dataset_versions(dataset_id,compute_fingerprint,check_fingerprint,published_at_us);
