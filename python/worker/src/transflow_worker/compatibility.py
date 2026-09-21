"""Explicit installed-package inspection, never executed at module import."""

from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, version

from ._version import SUPPORTED_PROTOCOL, __version__


class CompatibilityError(RuntimeError):
    """The installed distribution or peer protocol is incompatible."""


@dataclass(frozen=True, slots=True)
class CompatibilityReport:
    """Package compatibility evidence, not a claim of worker execution support."""

    distribution_version: str
    protocol_major: int
    protocol_minor: int
    wire_protocol_implemented: bool = True
    supported_operations: tuple[str, ...] = ("discover", "execute")


def check_compatibility(*, protocol_major: int = 1, protocol_minor: int = 0) -> CompatibilityReport:
    """Verify installed metadata, import identity and protocol major compatibility."""
    try:
        distribution_version = version("transflow")
    except PackageNotFoundError as exc:
        raise CompatibilityError(
            "The transflow distribution is not installed; install its local wheel with pip"
        ) from exc
    if distribution_version != __version__:
        raise CompatibilityError(
            f"Installed transflow {distribution_version} and imported {__version__} "
            "do not match; reinstall the transflow wheel"
        )
    try:
        import transflow
    except ImportError as exc:
        raise CompatibilityError(
            "The installed SDK cannot be imported; reinstall its wheel"
        ) from exc
    if transflow.__version__ != distribution_version:
        raise CompatibilityError("Imported SDK version differs from installed package metadata")
    if (
        transflow.PROTOCOL_VERSION.major,
        transflow.PROTOCOL_VERSION.minor,
    ) != SUPPORTED_PROTOCOL:
        raise CompatibilityError("SDK and worker protocol versions differ")
    try:
        transflow.require_protocol(transflow.ProtocolVersion(protocol_major, protocol_minor))
    except ValueError as exc:
        raise CompatibilityError(str(exc)) from exc
    return CompatibilityReport(distribution_version, *SUPPORTED_PROTOCOL)
