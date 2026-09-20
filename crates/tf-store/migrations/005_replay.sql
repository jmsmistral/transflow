-- Original execution evidence is retained independently of mutable branch heads.
-- Manifests retain metadata, not permanent artifact roots; expired data must fail replay.
CREATE TABLE replay_manifests (
    build_id TEXT PRIMARY KEY REFERENCES builds(id),
    manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json) AND length(manifest_json)<=1048576)
) STRICT;
CREATE TRIGGER replay_manifests_immutable BEFORE UPDATE ON replay_manifests BEGIN SELECT RAISE(ABORT,'immutable replay manifest'); END;
CREATE TRIGGER replay_manifests_append_only BEFORE DELETE ON replay_manifests BEGIN SELECT RAISE(ABORT,'immutable replay manifest'); END;
