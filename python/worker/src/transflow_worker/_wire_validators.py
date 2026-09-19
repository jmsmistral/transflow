"""Generated named validators backed by the shared runtime assertion engine."""

from .wire import validate_document


def validate_Uuid(value: object) -> None:
    """Assert the Uuid contract, including custom formats."""
    validate_document("Uuid", value)


def validate_Sha256(value: object) -> None:
    """Assert the Sha256 contract, including custom formats."""
    validate_document("Sha256", value)


def validate_Count(value: object) -> None:
    """Assert the Count contract, including custom formats."""
    validate_document("Count", value)


def validate_RelativePath(value: object) -> None:
    """Assert the RelativePath contract, including custom formats."""
    validate_document("RelativePath", value)


def validate_DatasetKey(value: object) -> None:
    """Assert the DatasetKey contract, including custom formats."""
    validate_document("DatasetKey", value)


def validate_LogicalType(value: object) -> None:
    """Assert the LogicalType contract, including custom formats."""
    validate_document("LogicalType", value)


def validate_Field(value: object) -> None:
    """Assert the Field contract, including custom formats."""
    validate_document("Field", value)


def validate_LogicalSchemaV1(value: object) -> None:
    """Assert the LogicalSchemaV1 contract, including custom formats."""
    validate_document("LogicalSchemaV1", value)


def validate_ScalarValue(value: object) -> None:
    """Assert the ScalarValue contract, including custom formats."""
    validate_document("ScalarValue", value)


def validate_WireValue(value: object) -> None:
    """Assert the WireValue contract, including custom formats."""
    validate_document("WireValue", value)


def validate_ArtifactFileV1(value: object) -> None:
    """Assert the ArtifactFileV1 contract, including custom formats."""
    validate_document("ArtifactFileV1", value)


def validate_ArtifactManifestV1(value: object) -> None:
    """Assert the ArtifactManifestV1 contract, including custom formats."""
    validate_document("ArtifactManifestV1", value)


def validate_CatalogEntryV1(value: object) -> None:
    """Assert the CatalogEntryV1 contract, including custom formats."""
    validate_document("CatalogEntryV1", value)


def validate_CatalogSnapshotV1(value: object) -> None:
    """Assert the CatalogSnapshotV1 contract, including custom formats."""
    validate_document("CatalogSnapshotV1", value)


def validate_ProtocolVersion(value: object) -> None:
    """Assert the ProtocolVersion contract, including custom formats."""
    validate_document("ProtocolVersion", value)


def validate_ControlMessageV1(value: object) -> None:
    """Assert the ControlMessageV1 contract, including custom formats."""
    validate_document("ControlMessageV1", value)


def validate_ControlFrameV1(value: object) -> None:
    """Assert the ControlFrameV1 contract, including custom formats."""
    validate_document("ControlFrameV1", value)
