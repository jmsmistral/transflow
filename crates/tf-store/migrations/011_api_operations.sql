-- A receipt is reserved before a side effect. An incomplete receipt is never retried
-- automatically after a crash: clients inspect retained builds before a new operation.
CREATE TABLE api_operations (
    id TEXT PRIMARY KEY,
    digest TEXT NOT NULL CHECK(length(digest)=64),
    response_json TEXT CHECK(response_json IS NULL OR (json_valid(response_json) AND length(response_json)<=33554432))
) STRICT;
CREATE TRIGGER api_operation_immutable BEFORE UPDATE ON api_operations WHEN OLD.response_json IS NOT NULL OR NEW.id!=OLD.id OR NEW.digest!=OLD.digest BEGIN SELECT RAISE(ABORT,'immutable API receipt'); END;
