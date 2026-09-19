-- Registry recovery and provider replica foundations.
CREATE TABLE catalog_mutations (
    id TEXT PRIMARY KEY,
    expected_old_digest TEXT NOT NULL CHECK(length(expected_old_digest)=64),
    expected_new_digest TEXT NOT NULL CHECK(length(expected_new_digest)=64),
    proposed_ids_json TEXT NOT NULL CHECK(json_valid(proposed_ids_json)),
    state TEXT NOT NULL CHECK(state IN ('PREPARED','REPLACED','INDEXED','CONFLICT')),
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json))
) STRICT;

CREATE TABLE external_registrations (
    source_snapshot_id TEXT NOT NULL REFERENCES source_snapshots(id),
    id TEXT NOT NULL,
    local_alias TEXT NOT NULL,
    provider_workspace_id TEXT NOT NULL,
    provider_dataset_id TEXT NOT NULL,
    selector_json TEXT NOT NULL CHECK(json_valid(selector_json)),
    policy_json TEXT NOT NULL CHECK(json_valid(policy_json)),
    PRIMARY KEY(source_snapshot_id,id),
    UNIQUE(source_snapshot_id,local_alias)
) STRICT;

CREATE TABLE foreign_versions (
    workspace_id TEXT NOT NULL,
    dataset_id TEXT NOT NULL,
    version_id TEXT NOT NULL,
    provenance_json TEXT NOT NULL CHECK(json_valid(provenance_json)),
    manifest_digest TEXT NOT NULL CHECK(length(manifest_digest)=64),
    availability TEXT NOT NULL,
    PRIMARY KEY(workspace_id,dataset_id,version_id)
) STRICT;

CREATE TABLE replicas (
    workspace_id TEXT NOT NULL,
    dataset_id TEXT NOT NULL,
    version_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
    provenance_json TEXT NOT NULL CHECK(json_valid(provenance_json)),
    copy_state TEXT NOT NULL CHECK(copy_state IN ('COPYING','VERIFIED','CORRUPT')),
    retention_json TEXT NOT NULL CHECK(json_valid(retention_json)),
    PRIMARY KEY(workspace_id,dataset_id,version_id),
    FOREIGN KEY(workspace_id,dataset_id,version_id) REFERENCES foreign_versions(workspace_id,dataset_id,version_id)
) STRICT;

CREATE TABLE read_leases (
    id TEXT PRIMARY KEY,
    version_id TEXT REFERENCES dataset_versions(id),
    artifact_digest TEXT REFERENCES artifacts(digest),
    owner_operation TEXT NOT NULL,
    renewed_at_us INTEGER NOT NULL,
    expires_at_us INTEGER NOT NULL CHECK(expires_at_us>=renewed_at_us),
    fence INTEGER NOT NULL CHECK(fence>=0),
    CHECK((version_id IS NULL)!=(artifact_digest IS NULL))
) STRICT;

CREATE INDEX catalog_mutations_state ON catalog_mutations(state);
CREATE INDEX read_leases_expiry ON read_leases(expires_at_us);
CREATE INDEX replicas_state ON replicas(copy_state);
