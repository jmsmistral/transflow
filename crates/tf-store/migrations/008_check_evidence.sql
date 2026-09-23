-- Immutable complete evaluations and separately protected diagnostic rows.
CREATE TABLE check_evaluations (
 id TEXT PRIMARY KEY,
 attempt_id TEXT NOT NULL REFERENCES attempts(id),
 envelope_json TEXT NOT NULL CHECK(json_valid(envelope_json) AND length(envelope_json)<=16777216),
 normalization TEXT NOT NULL
) STRICT;
CREATE TABLE check_samples (
 digest TEXT PRIMARY KEY,
 payload_json TEXT NOT NULL CHECK(json_valid(payload_json) AND length(payload_json)<=65536)
) STRICT;
CREATE TABLE check_evidence (
 result_id TEXT PRIMARY KEY REFERENCES check_results(id),
 evaluation_id TEXT NOT NULL REFERENCES check_evaluations(id)
) STRICT;
CREATE TABLE check_certificates (
 fingerprint TEXT PRIMARY KEY,
 result_id TEXT NOT NULL REFERENCES check_results(id),
 key_json TEXT NOT NULL CHECK(json_valid(key_json) AND length(key_json)<=1048576)
) STRICT;
CREATE TABLE check_reuses (
 result_id TEXT PRIMARY KEY REFERENCES check_results(id),
 original_result_id TEXT NOT NULL REFERENCES check_results(id),
 certificate_fingerprint TEXT NOT NULL REFERENCES check_certificates(fingerprint),
 reused_at_us INTEGER NOT NULL
) STRICT;
CREATE TABLE failed_check_candidates (
 attempt_id TEXT PRIMARY KEY REFERENCES attempts(id),
 artifact_digest TEXT NOT NULL REFERENCES artifacts(digest)
) STRICT;
CREATE TRIGGER retained_check_update BEFORE UPDATE ON check_results
WHEN EXISTS(SELECT 1 FROM check_evidence WHERE result_id=OLD.id) OR EXISTS(SELECT 1 FROM check_reuses WHERE result_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER retained_check_delete BEFORE DELETE ON check_results
WHEN EXISTS(SELECT 1 FROM check_evidence WHERE result_id=OLD.id) OR EXISTS(SELECT 1 FROM check_reuses WHERE result_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'retained check evidence'); END;
CREATE TRIGGER check_evaluations_update BEFORE UPDATE ON check_evaluations BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_evaluations_delete BEFORE DELETE ON check_evaluations BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_samples_update BEFORE UPDATE ON check_samples BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_samples_delete BEFORE DELETE ON check_samples BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_evidence_update BEFORE UPDATE ON check_evidence BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_evidence_delete BEFORE DELETE ON check_evidence BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_certificates_update BEFORE UPDATE ON check_certificates BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_certificates_delete BEFORE DELETE ON check_certificates BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_reuses_update BEFORE UPDATE ON check_reuses BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_reuses_delete BEFORE DELETE ON check_reuses BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER failed_check_candidates_update BEFORE UPDATE ON failed_check_candidates BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER failed_check_candidates_delete BEFORE DELETE ON failed_check_candidates BEGIN SELECT RAISE(ABORT,'immutable check evidence'); END;
CREATE TRIGGER check_definitions_update BEFORE UPDATE ON check_definitions BEGIN SELECT RAISE(ABORT,'immutable check definition'); END;
CREATE TRIGGER check_definitions_delete BEFORE DELETE ON check_definitions BEGIN SELECT RAISE(ABORT,'retained check definition'); END;
