-- Preserve historical bindings and bytes; foreign references no longer require a local replica.
CREATE TABLE job_inputs_v10 (
    job_id TEXT NOT NULL REFERENCES jobs(id),
    alias TEXT NOT NULL,
    version_id TEXT REFERENCES dataset_versions(id),
    parent_job_id TEXT REFERENCES jobs(id),
    binding_json TEXT NOT NULL CHECK(json_valid(binding_json)),
    foreign_workspace_id TEXT,
    foreign_dataset_id TEXT,
    foreign_version_id TEXT,
    CHECK((version_id IS NOT NULL)+(parent_job_id IS NOT NULL)+(foreign_version_id IS NOT NULL)=1),
    CHECK((foreign_workspace_id IS NULL)=(foreign_version_id IS NULL)),
    CHECK((foreign_dataset_id IS NULL)=(foreign_version_id IS NULL)),
    PRIMARY KEY(job_id,alias),
    FOREIGN KEY(foreign_workspace_id,foreign_dataset_id,foreign_version_id)
        REFERENCES foreign_versions(workspace_id,dataset_id,version_id)
) STRICT;
INSERT INTO job_inputs_v10 SELECT * FROM job_inputs;
DROP TABLE job_inputs;
ALTER TABLE job_inputs_v10 RENAME TO job_inputs;
CREATE INDEX job_inputs_foreign ON job_inputs(foreign_workspace_id,foreign_dataset_id,foreign_version_id);

-- Availability is established by a live provider lease, never by retained metadata.
UPDATE foreign_versions SET availability='METADATA_ONLY';
