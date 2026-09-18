"""Provisional packaging compatibility; no worker wire operations exist yet."""

from dataclasses import dataclass
from typing import Final


@dataclass(frozen=True, slots=True)
class ProtocolVersion:
    """Nonnegative major/minor protocol identity, independent of package version."""

    major: int
    minor: int

    def __post_init__(self) -> None:
        for name, value in (("major", self.major), ("minor", self.minor)):
            if type(value) is not int or value < 0:
                raise ValueError(f"Protocol {name} must be a nonnegative integer")


PROTOCOL_VERSION: Final = ProtocolVersion(0, 0)


class ProtocolCompatibilityError(ValueError):
    """A peer requests a protocol version this SDK does not implement."""

    def __init__(self, requested: ProtocolVersion, supported: ProtocolVersion) -> None:
        self.requested = requested
        self.supported = supported
        super().__init__(
            f"Protocol {requested.major}.{requested.minor} is incompatible with "
            f"supported {supported.major}.{supported.minor}; use a compatible transflow wheel"
        )


def require_protocol(requested: ProtocolVersion) -> None:
    """Require the exact bootstrap version until capability negotiation exists."""
    if requested != PROTOCOL_VERSION:
        raise ProtocolCompatibilityError(requested, PROTOCOL_VERSION)
