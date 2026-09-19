-- Initial runtime schema. UTC timestamps use signed microseconds.
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    singleton INTEGER NOT NULL DEFAULT 1 UNIQUE CHECK(singleton=1),
    root_identity TEXT NOT NULL,
    created_at_us INTEGER NOT NULL,
    runtime_owner TEXT
) STRICT;

CREATE TABLE datasets (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    created_at_us INTEGER NOT NULL,
    tombstoned_at_us INTEGER
) STRICT;

CREATE TABLE source_snapshots (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    code_digest TEXT NOT NULL,
    selector_json TEXT NOT NULL CHECK(json_valid(selector_json)),
    git_json TEXT CHECK(git_json IS NULL OR json_valid(git_json)),
    manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)),
    environment_json TEXT NOT NULL CHECK(json_valid(environment_json))
) STRICT;

CREATE TABLE dataset_definitions (
    source_snapshot_id TEXT NOT NULL REFERENCES source_snapshots(id),
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('transform','source','imported')),
    definition_json TEXT NOT NULL CHECK(json_valid(definition_json)),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    PRIMARY KEY(source_snapshot_id,dataset_id),
    UNIQUE(source_snapshot_id,path)
) STRICT;

CREATE TABLE transform_definitions (
    id TEXT PRIMARY KEY,
    source_snapshot_id TEXT NOT NULL,
    output_id TEXT NOT NULL,
    module TEXT NOT NULL,
    function TEXT NOT NULL,
    engine TEXT NOT NULL,
    declaration_json TEXT NOT NULL CHECK(json_valid(declaration_json)),
    semantic_fingerprint TEXT NOT NULL,
    UNIQUE(source_snapshot_id,output_id),
    FOREIGN KEY(source_snapshot_id,output_id) REFERENCES dataset_definitions(source_snapshot_id,dataset_id)
) STRICT;

CREATE TABLE dependency_edges (
    definition_id TEXT NOT NULL REFERENCES transform_definitions(id),
    alias TEXT NOT NULL,
    origin_workspace_id TEXT NOT NULL,
    origin_dataset_id TEXT NOT NULL,
    external_registration_id TEXT,
    selector_json TEXT NOT NULL CHECK(json_valid(selector_json)),
    stop_fallback INTEGER NOT NULL CHECK(stop_fallback IN (0,1)),
    role TEXT NOT NULL,
    checks_json TEXT NOT NULL CHECK(json_valid(checks_json)),
    PRIMARY KEY(definition_id,alias)
) STRICT;

CREATE TABLE data_branches (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    name TEXT NOT NULL,
    git_ref TEXT,
    revision INTEGER NOT NULL CHECK(revision>=0),
    deleted_at_us INTEGER,
    UNIQUE(workspace_id,name)
) STRICT;

CREATE TABLE branch_policies (
    source_snapshot_id TEXT NOT NULL REFERENCES source_snapshots(id),
    starting_branch TEXT NOT NULL,
    policy_json TEXT NOT NULL CHECK(json_valid(policy_json)),
    PRIMARY KEY(source_snapshot_id,starting_branch)
) STRICT;

CREATE TABLE artifacts (
    digest TEXT PRIMARY KEY CHECK(length(digest)=64),
    manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)),
    schema_json TEXT NOT NULL CHECK(json_valid(schema_json)),
    file_count INTEGER NOT NULL CHECK(file_count>=0),
    row_count INTEGER NOT NULL CHECK(row_count>=0),
    byte_count INTEGER NOT NULL CHECK(byte_count>=0),
    integrity_state TEXT NOT NULL
) STRICT;

CREATE TABLE build_plans (
    id TEXT PRIMARY KEY,
    disposition TEXT NOT NULL CHECK(disposition IN ('DRAFT','READY','ACCEPTED')),
    source_snapshot_id TEXT REFERENCES source_snapshots(id),
    candidate_json TEXT NOT NULL CHECK(json_valid(candidate_json)),
    registry_diff_json TEXT NOT NULL CHECK(json_valid(registry_diff_json)),
    context_json TEXT NOT NULL CHECK(json_valid(context_json)),
    targets_json TEXT NOT NULL CHECK(json_valid(targets_json)),
    bindings_json TEXT NOT NULL CHECK(json_valid(bindings_json)),
    guards_json TEXT NOT NULL CHECK(json_valid(guards_json)),
    digest TEXT NOT NULL,
    expires_at_us INTEGER
) STRICT;

CREATE TABLE builds (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL REFERENCES build_plans(id),
    occurrence_id TEXT REFERENCES schedule_occurrences(id) DEFERRABLE INITIALLY DEFERRED,
    trigger_json TEXT NOT NULL CHECK(json_valid(trigger_json)),
    requested_by TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('QUEUED','RUNNING','SUCCEEDED','FAILED','CANCELED','INTERRUPTED')),
    created_at_us INTEGER NOT NULL,
    finished_at_us INTEGER,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1))
) STRICT;

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    build_id TEXT NOT NULL REFERENCES builds(id),
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    definition_id TEXT REFERENCES transform_definitions(id),
    branch_id TEXT NOT NULL REFERENCES data_branches(id),
    state TEXT NOT NULL CHECK(state IN ('PLANNED','WAITING_DEPENDENCIES','QUEUED','STARTING','VALIDATING_INPUTS','RUNNING','MATERIALIZING','VALIDATING_OUTPUTS','COMMITTING','RETRY_WAIT','SUCCEEDED','CACHED','FAILED','BLOCKED','CANCELED','INTERRUPTED')),
    bindings_json TEXT NOT NULL CHECK(json_valid(bindings_json)),
    compute_key TEXT,
    UNIQUE(build_id,dataset_id),
    UNIQUE(id,dataset_id)
) STRICT;

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id),
    attempt_no INTEGER NOT NULL CHECK(attempt_no>0),
    process_json TEXT CHECK(process_json IS NULL OR json_valid(process_json)),
    session_id TEXT NOT NULL,
    fence INTEGER NOT NULL CHECK(fence>=0),
    state TEXT NOT NULL CHECK(state IN ('STARTING','VALIDATING_INPUTS','RUNNING','MATERIALIZING','VALIDATING_OUTPUTS','COMMITTING','SUCCEEDED','FAILED','CANCELED','INTERRUPTED')),
    failure_class TEXT,
    started_at_us INTEGER NOT NULL,
    finished_at_us INTEGER,
    log_path TEXT,
    UNIQUE(job_id,attempt_no)
) STRICT;

CREATE TABLE dataset_versions (
    id TEXT PRIMARY KEY,
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
    attempt_id TEXT REFERENCES attempts(id),
    import_id TEXT,
    source_snapshot_id TEXT NOT NULL REFERENCES source_snapshots(id),
    published_at_us INTEGER NOT NULL,
    compute_fingerprint TEXT NOT NULL,
    check_fingerprint TEXT NOT NULL,
    CHECK((attempt_id IS NULL)!=(import_id IS NULL)),
    UNIQUE(dataset_id,id)
) STRICT;

CREATE TABLE version_inputs (
    output_version_id TEXT NOT NULL REFERENCES dataset_versions(id),
    alias TEXT NOT NULL,
    origin_workspace_id TEXT NOT NULL,
    origin_dataset_id TEXT NOT NULL,
    origin_version_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    declared_branch_json TEXT NOT NULL CHECK(json_valid(declared_branch_json)),
    starting_branch TEXT NOT NULL,
    resolved_branch TEXT NOT NULL,
    role TEXT NOT NULL,
    resolution_json TEXT NOT NULL CHECK(json_valid(resolution_json)),
    PRIMARY KEY(output_version_id,alias)
) STRICT;

CREATE TABLE dataset_heads (
    branch_id TEXT NOT NULL REFERENCES data_branches(id),
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    version_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation>0),
    PRIMARY KEY(branch_id,dataset_id),
    FOREIGN KEY(dataset_id,version_id) REFERENCES dataset_versions(dataset_id,id)
) STRICT;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    causation_id TEXT,
    correlation_id TEXT,
    wall_time_us INTEGER NOT NULL
) STRICT;

CREATE TABLE head_changes (
    event_id TEXT PRIMARY KEY REFERENCES events(id),
    branch_id TEXT NOT NULL REFERENCES data_branches(id),
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    previous_version_id TEXT,
    new_version_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation>0),
    cause TEXT NOT NULL,
    committed_at_us INTEGER NOT NULL,
    FOREIGN KEY(dataset_id,previous_version_id) REFERENCES dataset_versions(dataset_id,id),
    FOREIGN KEY(dataset_id,new_version_id) REFERENCES dataset_versions(dataset_id,id),
    UNIQUE(branch_id,dataset_id,generation)
) STRICT;

CREATE TABLE phase_intervals (
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    sequence INTEGER NOT NULL CHECK(sequence>=0),
    phase TEXT NOT NULL,
    duration_ns INTEGER CHECK(duration_ns>=0),
    started_at_us INTEGER NOT NULL,
    finished_at_us INTEGER,
    PRIMARY KEY(attempt_id,sequence)
) STRICT;

CREATE TABLE job_inputs (
    job_id TEXT NOT NULL REFERENCES jobs(id),
    alias TEXT NOT NULL,
    version_id TEXT REFERENCES dataset_versions(id),
    parent_job_id TEXT REFERENCES jobs(id),
    binding_json TEXT NOT NULL CHECK(json_valid(binding_json)),
    CHECK((version_id IS NULL)!=(parent_job_id IS NULL)),
    PRIMARY KEY(job_id,alias)
) STRICT;

CREATE TABLE check_definitions (
    fingerprint TEXT PRIMARY KEY,
    ast_json TEXT NOT NULL CHECK(json_valid(ast_json)),
    semantic_version INTEGER NOT NULL CHECK(semantic_version>0),
    phase TEXT NOT NULL,
    target_alias TEXT,
    stable_id TEXT NOT NULL,
    policy_json TEXT NOT NULL CHECK(json_valid(policy_json))
) STRICT;

CREATE TABLE check_results (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    subject_json TEXT NOT NULL CHECK(json_valid(subject_json)),
    definition_fingerprint TEXT NOT NULL REFERENCES check_definitions(fingerprint),
    outcome TEXT NOT NULL CHECK(outcome IN ('PASS','VIOLATION','ERROR','SKIPPED')),
    metrics_json TEXT NOT NULL CHECK(json_valid(metrics_json)),
    started_at_us INTEGER NOT NULL,
    finished_at_us INTEGER,
    sample_ref TEXT
) STRICT;

CREATE TABLE validation_reuse (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id),
    original_result_id TEXT NOT NULL REFERENCES check_results(id),
    certificate_fingerprint TEXT NOT NULL
) STRICT;

CREATE TABLE publication_intents (
    attempt_id TEXT PRIMARY KEY REFERENCES attempts(id),
    planned_version_id TEXT NOT NULL UNIQUE,
    artifact_digest TEXT NOT NULL REFERENCES artifacts(digest),
    expected_head_generation INTEGER NOT NULL CHECK(expected_head_generation>=0),
    session_id TEXT NOT NULL,
    fence INTEGER NOT NULL CHECK(fence>=0),
    state TEXT NOT NULL CHECK(state IN ('PREPARED','COMMITTED','ABANDONED'))
) STRICT;

CREATE TABLE write_reservations (
    branch_id TEXT NOT NULL REFERENCES data_branches(id),
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    build_id TEXT NOT NULL REFERENCES builds(id),
    session_id TEXT NOT NULL,
    fence INTEGER NOT NULL CHECK(fence>=0),
    acquired_at_us INTEGER NOT NULL,
    PRIMARY KEY(branch_id,dataset_id)
) STRICT;

CREATE TABLE cache_entries (
    branch_id TEXT NOT NULL REFERENCES data_branches(id),
    compute_key TEXT NOT NULL,
    dataset_id TEXT NOT NULL REFERENCES datasets(id),
    version_id TEXT NOT NULL,
    certificates_json TEXT NOT NULL CHECK(json_valid(certificates_json)),
    integrity_requirement TEXT NOT NULL,
    invalid_reason TEXT,
    PRIMARY KEY(branch_id,compute_key,dataset_id),
    FOREIGN KEY(dataset_id,version_id) REFERENCES dataset_versions(dataset_id,id)
) STRICT;

CREATE TABLE schedules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    active_revision INTEGER,
    paused INTEGER NOT NULL CHECK(paused IN (0,1)),
    deleted_at_us INTEGER,
    FOREIGN KEY(id,active_revision) REFERENCES schedule_revisions(schedule_id,revision) DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE TABLE schedule_revisions (
    schedule_id TEXT NOT NULL REFERENCES schedules(id),
    revision INTEGER NOT NULL CHECK(revision>0),
    trigger_json TEXT NOT NULL CHECK(json_valid(trigger_json)),
    build_template_json TEXT NOT NULL CHECK(json_valid(build_template_json)),
    policies_json TEXT NOT NULL CHECK(json_valid(policies_json)),
    author TEXT NOT NULL,
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(schedule_id,revision)
) STRICT;

CREATE TABLE trigger_tokens (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    leaf_id TEXT NOT NULL,
    event_id TEXT REFERENCES events(id),
    tick_id TEXT,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    seen_at_us INTEGER NOT NULL,
    expires_at_us INTEGER,
    consumed_by TEXT REFERENCES schedule_occurrences(id),
    CHECK((event_id IS NULL)!=(tick_id IS NULL)),
    FOREIGN KEY(schedule_id,revision) REFERENCES schedule_revisions(schedule_id,revision)
) STRICT;

CREATE TABLE schedule_occurrences (
    id TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    evidence_digest TEXT NOT NULL,
    logical_fire_at_us INTEGER NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    disposition TEXT NOT NULL,
    build_id TEXT REFERENCES builds(id) DEFERRABLE INITIALLY DEFERRED,
    UNIQUE(schedule_id,revision,evidence_digest),
    FOREIGN KEY(schedule_id,revision) REFERENCES schedule_revisions(schedule_id,revision)
) STRICT;

CREATE TABLE outbox_deliveries (
    event_id TEXT NOT NULL REFERENCES events(id),
    target TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    schema_version INTEGER NOT NULL CHECK(schema_version>0),
    attempts INTEGER NOT NULL CHECK(attempts>=0),
    next_attempt_at_us INTEGER,
    acknowledged_at_us INTEGER,
    PRIMARY KEY(event_id,target)
) STRICT;

CREATE TABLE graph_views (
    id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK(revision>=0),
    format_version INTEGER NOT NULL CHECK(format_version>0),
    view_json TEXT NOT NULL CHECK(json_valid(view_json)),
    context_json TEXT NOT NULL CHECK(json_valid(context_json)),
    saved_at_us INTEGER NOT NULL
) STRICT;

CREATE TABLE column_lineage (
    id TEXT PRIMARY KEY,
    source_snapshot_id TEXT NOT NULL REFERENCES source_snapshots(id),
    definition_id TEXT REFERENCES transform_definitions(id),
    version_id TEXT REFERENCES dataset_versions(id),
    output_column TEXT NOT NULL,
    source_columns_json TEXT NOT NULL CHECK(json_valid(source_columns_json)),
    kind TEXT NOT NULL,
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json))
) STRICT;

CREATE TABLE pins (
    id TEXT PRIMARY KEY,
    artifact_digest TEXT REFERENCES artifacts(digest),
    version_id TEXT REFERENCES dataset_versions(id),
    source_snapshot_id TEXT REFERENCES source_snapshots(id),
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    expires_at_us INTEGER,
    CHECK((artifact_digest IS NOT NULL)+(version_id IS NOT NULL)+(source_snapshot_id IS NOT NULL)=1)
) STRICT;

CREATE TABLE audit_log (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation TEXT NOT NULL,
    evidence_json TEXT NOT NULL CHECK(json_valid(evidence_json)),
    wall_time_us INTEGER NOT NULL
) STRICT;

CREATE TRIGGER source_snapshots_immutable BEFORE UPDATE ON source_snapshots BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER dataset_definitions_immutable BEFORE UPDATE ON dataset_definitions BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER transform_definitions_immutable BEFORE UPDATE ON transform_definitions BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER dependency_edges_immutable BEFORE UPDATE ON dependency_edges BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER branch_policies_immutable BEFORE UPDATE ON branch_policies BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER artifacts_immutable BEFORE UPDATE ON artifacts BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER dataset_versions_immutable BEFORE UPDATE ON dataset_versions BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER version_inputs_immutable BEFORE UPDATE ON version_inputs BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER events_immutable BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER head_changes_immutable BEFORE UPDATE ON head_changes BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER schedule_revisions_immutable BEFORE UPDATE ON schedule_revisions BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER check_definitions_immutable BEFORE UPDATE ON check_definitions BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;

CREATE TRIGGER audit_log_immutable BEFORE UPDATE ON audit_log BEGIN SELECT RAISE(ABORT, 'immutable evidence'); END;
CREATE TRIGGER events_append_only BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER head_changes_append_only BEFORE DELETE ON head_changes BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER audit_log_append_only BEFORE DELETE ON audit_log BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;

CREATE INDEX datasets_workspace ON datasets(workspace_id);
CREATE INDEX versions_dataset_time ON dataset_versions(dataset_id,published_at_us);
CREATE INDEX jobs_build_state ON jobs(build_id,state);
CREATE INDEX attempts_job ON attempts(job_id,attempt_no);
CREATE INDEX events_type_sequence ON events(type,sequence);
CREATE INDEX outbox_due ON outbox_deliveries(next_attempt_at_us) WHERE acknowledged_at_us IS NULL;
CREATE INDEX pins_expiry ON pins(expires_at_us);

CREATE TRIGGER heads_generation BEFORE UPDATE ON dataset_heads
WHEN NEW.generation <= OLD.generation OR NEW.dataset_id != OLD.dataset_id OR NEW.branch_id != OLD.branch_id
BEGIN SELECT RAISE(ABORT, 'head generation must advance without identity change'); END;
CREATE TRIGGER version_attempt_dataset BEFORE INSERT ON dataset_versions
WHEN NEW.attempt_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM attempts a JOIN jobs j ON j.id=a.job_id
    WHERE a.id=NEW.attempt_id AND j.dataset_id=NEW.dataset_id
)
BEGIN SELECT RAISE(ABORT, 'attempt must produce the version dataset'); END;
CREATE TRIGGER attempts_terminal BEFORE UPDATE ON attempts
WHEN OLD.state IN ('SUCCEEDED','FAILED','CANCELED','INTERRUPTED')
BEGIN SELECT RAISE(ABORT, 'terminal attempt is immutable'); END;
