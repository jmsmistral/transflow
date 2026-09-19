-- Synthetic harness schema only; not a Transflow catalogue or migration.
CREATE TABLE fixture_artifact (name TEXT PRIMARY KEY);
INSERT INTO fixture_artifact VALUES ('last-good'), ('candidate');
CREATE TABLE fixture_head (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    artifact TEXT NOT NULL REFERENCES fixture_artifact(name)
);
INSERT INTO fixture_head VALUES (1, 'last-good');
