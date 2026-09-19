# Synthetic fixtures

`harness.sql` is T007's tiny SQLite transaction fixture. It contains an artifact
lookup and one head with a foreign key, solely to test real commit, rollback and
process-death behaviour in the [integration harness](../tests/README.md).

It is not the future Transflow catalogue schema, a migration or an example user
workspace. Public fixtures must remain synthetic and must not copy the ignored
reference screenshots or their names/data. Versioned protocol and engine fixtures
will be added alongside their actual implementations.
