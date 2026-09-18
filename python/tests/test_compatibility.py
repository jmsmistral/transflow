"""Positive, invalid-version and package mismatch tests for the bootstrap API."""

import sys
from dataclasses import FrozenInstanceError
from importlib.metadata import PackageNotFoundError
from io import StringIO
from typing import cast

import pytest
import transflow
from transflow import (
    PROTOCOL_VERSION,
    ProtocolCompatibilityError,
    ProtocolVersion,
    require_protocol,
)
from transflow_worker import cli, compatibility


def test_protocol_identity_is_immutable() -> None:
    with pytest.raises(FrozenInstanceError):
        # Deliberately test a runtime mutation that static checking also rejects.
        PROTOCOL_VERSION.major = 1  # type: ignore[misc]
    require_protocol(ProtocolVersion(0, 0))


@pytest.mark.parametrize("component", [-1, True, 1.5, "0", None])
def test_invalid_protocol_components(component: object) -> None:
    # Deliberately simulate an untyped caller supplying an invalid value.
    with pytest.raises(ValueError, match="nonnegative integer"):
        ProtocolVersion(cast(int, component), 0)


@pytest.mark.parametrize("version", [ProtocolVersion(1, 0), ProtocolVersion(0, 1)])
def test_unknown_major_or_minor_fails_with_context(version: ProtocolVersion) -> None:
    with pytest.raises(ProtocolCompatibilityError) as error:
        require_protocol(version)
    assert error.value.requested == version
    assert error.value.supported == PROTOCOL_VERSION
    assert "compatible transflow wheel" in str(error.value)


def test_missing_distribution_preserves_error_source(monkeypatch: pytest.MonkeyPatch) -> None:
    def missing(name: str) -> str:
        raise PackageNotFoundError(name)

    monkeypatch.setattr(compatibility, "version", missing)
    with pytest.raises(compatibility.CompatibilityError, match="not installed") as error:
        compatibility.check_compatibility()
    assert isinstance(error.value.__cause__, PackageNotFoundError)


def test_metadata_mismatch_fails_before_sdk_use(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(compatibility, "version", lambda name: "0.0.0.dev1")
    with pytest.raises(compatibility.CompatibilityError, match="do not match"):
        compatibility.check_compatibility()


def test_imported_version_must_match_metadata(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(compatibility, "version", lambda name: "0.0.0.dev0")
    monkeypatch.setattr(transflow, "__version__", "0.0.0.dev1")
    with pytest.raises(compatibility.CompatibilityError, match="differs from installed"):
        compatibility.check_compatibility()


def test_sdk_protocol_must_match_worker(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(compatibility, "version", lambda name: "0.0.0.dev0")
    monkeypatch.setattr(transflow, "PROTOCOL_VERSION", ProtocolVersion(1, 0))
    with pytest.raises(compatibility.CompatibilityError, match="protocol versions differ"):
        compatibility.check_compatibility()


class ClosedOutput(StringIO):
    def write(self, text: str, /) -> int:
        raise BrokenPipeError("synthetic closed output")


def test_closed_stdout_reports_context(monkeypatch: pytest.MonkeyPatch) -> None:
    diagnostic = StringIO()
    monkeypatch.setattr(sys, "stdout", ClosedOutput())
    monkeypatch.setattr(sys, "stderr", diagnostic)
    assert cli.main(["--help"]) == 1
    assert "Could not write worker diagnostic output" in diagnostic.getvalue()


def test_closed_stderr_preserves_failure_exit(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sys, "stdout", ClosedOutput())
    monkeypatch.setattr(sys, "stderr", ClosedOutput())
    assert cli.main([]) == 1
