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


def validate_GraphNodeV1(value: object) -> None:
    """Assert the GraphNodeV1 contract, including custom formats."""
    validate_document("GraphNodeV1", value)


def validate_GraphEdgeV1(value: object) -> None:
    """Assert the GraphEdgeV1 contract, including custom formats."""
    validate_document("GraphEdgeV1", value)


def validate_GraphResultV1(value: object) -> None:
    """Assert the GraphResultV1 contract, including custom formats."""
    validate_document("GraphResultV1", value)


def validate_FreshnessStatusV1(value: object) -> None:
    """Assert the FreshnessStatusV1 contract, including custom formats."""
    validate_document("FreshnessStatusV1", value)


def validate_PlanResultV1(value: object) -> None:
    """Assert the PlanResultV1 contract, including custom formats."""
    validate_document("PlanResultV1", value)


def validate_WhyResultV1(value: object) -> None:
    """Assert the WhyResultV1 contract, including custom formats."""
    validate_document("WhyResultV1", value)


def validate_ResourcePolicyV1(value: object) -> None:
    """Assert the ResourcePolicyV1 contract, including custom formats."""
    validate_document("ResourcePolicyV1", value)


def validate_PolarsExecutionRequestV1(value: object) -> None:
    """Assert the PolarsExecutionRequestV1 contract, including custom formats."""
    validate_document("PolarsExecutionRequestV1", value)


def validate_ExpectationInputRef(value: object) -> None:
    """Assert the ExpectationInputRef contract, including custom formats."""
    validate_document("ExpectationInputRef", value)


def validate_ExpectationMetric(value: object) -> None:
    """Assert the ExpectationMetric contract, including custom formats."""
    validate_document("ExpectationMetric", value)


def validate_ExpectationValue(value: object) -> None:
    """Assert the ExpectationValue contract, including custom formats."""
    validate_document("ExpectationValue", value)


def validate_ExpectationScalar(value: object) -> None:
    """Assert the ExpectationScalar contract, including custom formats."""
    validate_document("ExpectationScalar", value)


def validate_ExpectationNode(value: object) -> None:
    """Assert the ExpectationNode contract, including custom formats."""
    validate_document("ExpectationNode", value)


def validate_ExpectationAstV1(value: object) -> None:
    """Assert the ExpectationAstV1 contract, including custom formats."""
    validate_document("ExpectationAstV1", value)


def validate_CheckQueryV1(value: object) -> None:
    """Assert the CheckQueryV1 contract, including custom formats."""
    validate_document("CheckQueryV1", value)


def validate_CheckEvaluationRequestV1(value: object) -> None:
    """Assert the CheckEvaluationRequestV1 contract, including custom formats."""
    validate_document("CheckEvaluationRequestV1", value)


def validate_CheckAggregatesV1(value: object) -> None:
    """Assert the CheckAggregatesV1 contract, including custom formats."""
    validate_document("CheckAggregatesV1", value)


def validate_CheckMetricV1(value: object) -> None:
    """Assert the CheckMetricV1 contract, including custom formats."""
    validate_document("CheckMetricV1", value)


def validate_CheckEvaluationResultV1(value: object) -> None:
    """Assert the CheckEvaluationResultV1 contract, including custom formats."""
    validate_document("CheckEvaluationResultV1", value)


def validate_CheckSamplePolicyV1(value: object) -> None:
    """Assert the CheckSamplePolicyV1 contract, including custom formats."""
    validate_document("CheckSamplePolicyV1", value)


def validate_CheckSampleV1(value: object) -> None:
    """Assert the CheckSampleV1 contract, including custom formats."""
    validate_document("CheckSampleV1", value)


def validate_CheckSampleQueryV1(value: object) -> None:
    """Assert the CheckSampleQueryV1 contract, including custom formats."""
    validate_document("CheckSampleQueryV1", value)


def validate_ExecutionJsonV1(value: object) -> None:
    """Assert the ExecutionJsonV1 contract, including custom formats."""
    validate_document("ExecutionJsonV1", value)


def validate_ExecutionResultV1(value: object) -> None:
    """Assert the ExecutionResultV1 contract, including custom formats."""
    validate_document("ExecutionResultV1", value)


def validate_ExternalEntryV1(value: object) -> None:
    """Assert the ExternalEntryV1 contract, including custom formats."""
    validate_document("ExternalEntryV1", value)


def validate_ExternalResultV1(value: object) -> None:
    """Assert the ExternalResultV1 contract, including custom formats."""
    validate_document("ExternalResultV1", value)


def validate_ImportPublicationResultV1(value: object) -> None:
    """Assert the ImportPublicationResultV1 contract, including custom formats."""
    validate_document("ImportPublicationResultV1", value)


def validate_ApiContextV1(value: object) -> None:
    """Assert the ApiContextV1 contract, including custom formats."""
    validate_document("ApiContextV1", value)


def validate_ApiErrorV1(value: object) -> None:
    """Assert the ApiErrorV1 contract, including custom formats."""
    validate_document("ApiErrorV1", value)


def validate_ApiPreparationV1(value: object) -> None:
    """Assert the ApiPreparationV1 contract, including custom formats."""
    validate_document("ApiPreparationV1", value)


def validate_ApiSelectionV1(value: object) -> None:
    """Assert the ApiSelectionV1 contract, including custom formats."""
    validate_document("ApiSelectionV1", value)


def validate_ApiBuildRequestV1(value: object) -> None:
    """Assert the ApiBuildRequestV1 contract, including custom formats."""
    validate_document("ApiBuildRequestV1", value)


def validate_ApiAcceptedV1(value: object) -> None:
    """Assert the ApiAcceptedV1 contract, including custom formats."""
    validate_document("ApiAcceptedV1", value)


def validate_ApiCanceledV1(value: object) -> None:
    """Assert the ApiCanceledV1 contract, including custom formats."""
    validate_document("ApiCanceledV1", value)


def validate_ApiDatasetV1(value: object) -> None:
    """Assert the ApiDatasetV1 contract, including custom formats."""
    validate_document("ApiDatasetV1", value)


def validate_ApiDatasetInspectionV1(value: object) -> None:
    """Assert the ApiDatasetInspectionV1 contract, including custom formats."""
    validate_document("ApiDatasetInspectionV1", value)


def validate_ApiDatasetsV1(value: object) -> None:
    """Assert the ApiDatasetsV1 contract, including custom formats."""
    validate_document("ApiDatasetsV1", value)


def validate_ApiVersionsV1(value: object) -> None:
    """Assert the ApiVersionsV1 contract, including custom formats."""
    validate_document("ApiVersionsV1", value)


def validate_ApiSourceV1(value: object) -> None:
    """Assert the ApiSourceV1 contract, including custom formats."""
    validate_document("ApiSourceV1", value)


def validate_ApiLineageV1(value: object) -> None:
    """Assert the ApiLineageV1 contract, including custom formats."""
    validate_document("ApiLineageV1", value)


def validate_ApiEmptyV1(value: object) -> None:
    """Assert the ApiEmptyV1 contract, including custom formats."""
    validate_document("ApiEmptyV1", value)


def validate_ApiExchangeV1(value: object) -> None:
    """Assert the ApiExchangeV1 contract, including custom formats."""
    validate_document("ApiExchangeV1", value)


def validate_ApiLaunchedV1(value: object) -> None:
    """Assert the ApiLaunchedV1 contract, including custom formats."""
    validate_document("ApiLaunchedV1", value)


def validate_ApiSessionV1(value: object) -> None:
    """Assert the ApiSessionV1 contract, including custom formats."""
    validate_document("ApiSessionV1", value)


def validate_ApiVerifiedV1(value: object) -> None:
    """Assert the ApiVerifiedV1 contract, including custom formats."""
    validate_document("ApiVerifiedV1", value)


def validate_ApiHealthV1(value: object) -> None:
    """Assert the ApiHealthV1 contract, including custom formats."""
    validate_document("ApiHealthV1", value)


def validate_ApiCapabilitiesV1(value: object) -> None:
    """Assert the ApiCapabilitiesV1 contract, including custom formats."""
    validate_document("ApiCapabilitiesV1", value)


def validate_ApiCatalogCommandV1(value: object) -> None:
    """Assert the ApiCatalogCommandV1 contract, including custom formats."""
    validate_document("ApiCatalogCommandV1", value)


def validate_ApiBranchCommandV1(value: object) -> None:
    """Assert the ApiBranchCommandV1 contract, including custom formats."""
    validate_document("ApiBranchCommandV1", value)


def validate_ApiExternalCommandV1(value: object) -> None:
    """Assert the ApiExternalCommandV1 contract, including custom formats."""
    validate_document("ApiExternalCommandV1", value)


def validate_ApiReadV1(value: object) -> None:
    """Assert the ApiReadV1 contract, including custom formats."""
    validate_document("ApiReadV1", value)


def validate_ApiMetadataPageV1(value: object) -> None:
    """Assert the ApiMetadataPageV1 contract, including custom formats."""
    validate_document("ApiMetadataPageV1", value)


def validate_ApiEventV1(value: object) -> None:
    """Assert the ApiEventV1 contract, including custom formats."""
    validate_document("ApiEventV1", value)


def validate_ApiEventsV1(value: object) -> None:
    """Assert the ApiEventsV1 contract, including custom formats."""
    validate_document("ApiEventsV1", value)


def validate_ApiPreviewRequestV1(value: object) -> None:
    """Assert the ApiPreviewRequestV1 contract, including custom formats."""
    validate_document("ApiPreviewRequestV1", value)


def validate_ApiPreviewCellV1(value: object) -> None:
    """Assert the ApiPreviewCellV1 contract, including custom formats."""
    validate_document("ApiPreviewCellV1", value)


def validate_ApiPreviewV1(value: object) -> None:
    """Assert the ApiPreviewV1 contract, including custom formats."""
    validate_document("ApiPreviewV1", value)


def validate_JsonObject(value: object) -> None:
    """Assert the JsonObject contract, including custom formats."""
    validate_document("JsonObject", value)


def validate_ScratchpadBindingV1(value: object) -> None:
    """Assert the ScratchpadBindingV1 contract, including custom formats."""
    validate_document("ScratchpadBindingV1", value)


def validate_ScratchpadRequestV1(value: object) -> None:
    """Assert the ScratchpadRequestV1 contract, including custom formats."""
    validate_document("ScratchpadRequestV1", value)


def validate_ScratchpadResultV1(value: object) -> None:
    """Assert the ScratchpadResultV1 contract, including custom formats."""
    validate_document("ScratchpadResultV1", value)


def validate_QueryLimitsV1(value: object) -> None:
    """Assert the QueryLimitsV1 contract, including custom formats."""
    validate_document("QueryLimitsV1", value)


def validate_QueryExecutionRequestV1(value: object) -> None:
    """Assert the QueryExecutionRequestV1 contract, including custom formats."""
    validate_document("QueryExecutionRequestV1", value)


def validate_ApiQueryRequestV1(value: object) -> None:
    """Assert the ApiQueryRequestV1 contract, including custom formats."""
    validate_document("ApiQueryRequestV1", value)


def validate_ApiQueryV1(value: object) -> None:
    """Assert the ApiQueryV1 contract, including custom formats."""
    validate_document("ApiQueryV1", value)


def validate_ApiQueryResultsV1(value: object) -> None:
    """Assert the ApiQueryResultsV1 contract, including custom formats."""
    validate_document("ApiQueryResultsV1", value)


def validate_ApiExactRatioV1(value: object) -> None:
    """Assert the ApiExactRatioV1 contract, including custom formats."""
    validate_document("ApiExactRatioV1", value)


def validate_ApiStateCountV1(value: object) -> None:
    """Assert the ApiStateCountV1 contract, including custom formats."""
    validate_document("ApiStateCountV1", value)


def validate_ApiPhaseIntervalV1(value: object) -> None:
    """Assert the ApiPhaseIntervalV1 contract, including custom formats."""
    validate_document("ApiPhaseIntervalV1", value)


def validate_ApiAttemptTimingV1(value: object) -> None:
    """Assert the ApiAttemptTimingV1 contract, including custom formats."""
    validate_document("ApiAttemptTimingV1", value)


def validate_ApiJobTimingV1(value: object) -> None:
    """Assert the ApiJobTimingV1 contract, including custom formats."""
    validate_document("ApiJobTimingV1", value)


def validate_ApiExecutionTimelineV1(value: object) -> None:
    """Assert the ApiExecutionTimelineV1 contract, including custom formats."""
    validate_document("ApiExecutionTimelineV1", value)


def validate_ApiAttemptV1(value: object) -> None:
    """Assert the ApiAttemptV1 contract, including custom formats."""
    validate_document("ApiAttemptV1", value)


def validate_ApiExecutionMetricsV1(value: object) -> None:
    """Assert the ApiExecutionMetricsV1 contract, including custom formats."""
    validate_document("ApiExecutionMetricsV1", value)


def validate_ApiLineageNodeV1(value: object) -> None:
    """Assert the ApiLineageNodeV1 contract, including custom formats."""
    validate_document("ApiLineageNodeV1", value)


def validate_ViewPositionV1(value: object) -> None:
    """Assert the ViewPositionV1 contract, including custom formats."""
    validate_document("ViewPositionV1", value)


def validate_ViewDatasetV1(value: object) -> None:
    """Assert the ViewDatasetV1 contract, including custom formats."""
    validate_document("ViewDatasetV1", value)


def validate_ViewSelectorV1(value: object) -> None:
    """Assert the ViewSelectorV1 contract, including custom formats."""
    validate_document("ViewSelectorV1", value)


def validate_GraphViewV1(value: object) -> None:
    """Assert the GraphViewV1 contract, including custom formats."""
    validate_document("GraphViewV1", value)


def validate_ApiViewsV1(value: object) -> None:
    """Assert the ApiViewsV1 contract, including custom formats."""
    validate_document("ApiViewsV1", value)
