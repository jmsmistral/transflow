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


def validate_DiagnosticText(value: object) -> None:
    """Assert the DiagnosticText contract, including custom formats."""
    validate_document("DiagnosticText", value)


def validate_SourcePositionV1(value: object) -> None:
    """Assert the SourcePositionV1 contract, including custom formats."""
    validate_document("SourcePositionV1", value)


def validate_SourceRangeV1(value: object) -> None:
    """Assert the SourceRangeV1 contract, including custom formats."""
    validate_document("SourceRangeV1", value)


def validate_RequestContextV1(value: object) -> None:
    """Assert the RequestContextV1 contract, including custom formats."""
    validate_document("RequestContextV1", value)


def validate_DiagnosticV1(value: object) -> None:
    """Assert the DiagnosticV1 contract, including custom formats."""
    validate_document("DiagnosticV1", value)


def validate_CliEnvelopeV1(value: object) -> None:
    """Assert the CliEnvelopeV1 contract, including custom formats."""
    validate_document("CliEnvelopeV1", value)


def validate_DiscoveryRefV1(value: object) -> None:
    """Assert the DiscoveryRefV1 contract, including custom formats."""
    validate_document("DiscoveryRefV1", value)


def validate_DeclarationCheckV1(value: object) -> None:
    """Assert the DeclarationCheckV1 contract, including custom formats."""
    validate_document("DeclarationCheckV1", value)


def validate_DeclarationInputV1(value: object) -> None:
    """Assert the DeclarationInputV1 contract, including custom formats."""
    validate_document("DeclarationInputV1", value)


def validate_DeclarationV1(value: object) -> None:
    """Assert the DeclarationV1 contract, including custom formats."""
    validate_document("DeclarationV1", value)


def validate_DiscoveryRequestV1(value: object) -> None:
    """Assert the DiscoveryRequestV1 contract, including custom formats."""
    validate_document("DiscoveryRequestV1", value)


def validate_DiscoveryResultV1(value: object) -> None:
    """Assert the DiscoveryResultV1 contract, including custom formats."""
    validate_document("DiscoveryResultV1", value)


def validate_DiscoveryDiagnosticV1(value: object) -> None:
    """Assert the DiscoveryDiagnosticV1 contract, including custom formats."""
    validate_document("DiscoveryDiagnosticV1", value)


def validate_CatalogAliasV1(value: object) -> None:
    """Assert the CatalogAliasV1 contract, including custom formats."""
    validate_document("CatalogAliasV1", value)


def validate_PreparationResultV1(value: object) -> None:
    """Assert the PreparationResultV1 contract, including custom formats."""
    validate_document("PreparationResultV1", value)


def validate_CatalogResultV1(value: object) -> None:
    """Assert the CatalogResultV1 contract, including custom formats."""
    validate_document("CatalogResultV1", value)


def validate_ImportPreparationResultV1(value: object) -> None:
    """Assert the ImportPreparationResultV1 contract, including custom formats."""
    validate_document("ImportPreparationResultV1", value)


def validate_ImportStagingManifestV1(value: object) -> None:
    """Assert the ImportStagingManifestV1 contract, including custom formats."""
    validate_document("ImportStagingManifestV1", value)


def validate_BranchResultV1(value: object) -> None:
    """Assert the BranchResultV1 contract, including custom formats."""
    validate_document("BranchResultV1", value)
