"""Worker protocol major compatibility; sessions negotiate required capabilities."""

from dataclasses import dataclass
from typing import Final


@dataclass(frozen=True, slots=True)
class ProtocolVersion:
    """Nonnegative major/minor protocol identity, independent of package version."""

    major: int
    minor: int

    def __post_init__(self) -> None:
        for name, value in (("major", self.major), ("minor", self.minor)):
            if type(value) is not int or not 0 <= value <= 4294967295:
                raise ValueError(
                    f"Protocol {name} must be a nonnegative integer no greater than 4294967295"
                )


PROTOCOL_VERSION: Final = ProtocolVersion(1, 0)


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
    """Check the major before session-level capability negotiation."""
    if requested.major != PROTOCOL_VERSION.major:
        raise ProtocolCompatibilityError(requested, PROTOCOL_VERSION)
