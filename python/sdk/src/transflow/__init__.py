"""Typed SDK bootstrap with no workspace, engine or process initialization."""

from ._version import __version__ as __version__
from .compatibility import (
    PROTOCOL_VERSION as PROTOCOL_VERSION,
)
from .compatibility import (
    ProtocolCompatibilityError as ProtocolCompatibilityError,
)
from .compatibility import (
    ProtocolVersion as ProtocolVersion,
)
from .compatibility import (
    require_protocol as require_protocol,
)

__all__ = [
    "PROTOCOL_VERSION",
    "ProtocolCompatibilityError",
    "ProtocolVersion",
    "__version__",
    "require_protocol",
]
