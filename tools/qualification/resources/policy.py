"""T011 feasibility policy; not coordinator admission or production supervision."""

from __future__ import annotations

import math
from dataclasses import dataclass


class UnsupportedLimit(ValueError):
    """A requested enforcement mechanism has not been qualified."""


@dataclass(frozen=True)
class Limits:
    compute: float = 3600
    validation: float = 3600
    interactive: float = 30
    memory_budget: int | None = None
    hard_memory: int | None = None

    def __post_init__(self):
        for name in ("compute", "validation", "interactive"):
            value = getattr(self, name)
            if isinstance(value, bool) or not math.isfinite(value) or value < 0:
                raise ValueError(
                    f"{name} timeout must be finite nonnegative seconds; 0 disables it"
                )
        for name in ("memory_budget", "hard_memory"):
            value = getattr(self, name)
            if value is not None and (type(value) is not int or value <= 0):
                raise ValueError(f"{name} must be a positive integer byte count")
        if self.hard_memory is not None:
            raise UnsupportedLimit(
                "Process hard memory limits are not supported by this prototype on any platform. "
                "Linux requires separately qualified cgroup support; macOS has no such backend. "
                "Use explicit admission reservations or engine limits with their documented scope."
            )

    def admits(self, estimate: int, reserved: int = 0) -> bool:
        if type(estimate) is not int or type(reserved) is not int or min(estimate, reserved) < 0:
            raise ValueError("Memory estimates and reservations must be nonnegative integer bytes")
        return self.memory_budget is None or reserved + estimate <= self.memory_budget

    def start(self, phase: str, now: float, *, name="synthetic/check", source="workspace"):
        if phase not in ("compute", "validation", "interactive"):
            raise ValueError("Unknown phase")
        return Deadline(phase, now, getattr(self, phase), name, source)


@dataclass(frozen=True)
class Deadline:
    phase: str
    started: float
    seconds: float
    name: str
    source: str

    def expired(self, now: float) -> bool:
        return self.seconds > 0 and now - self.started >= self.seconds

    def diagnostic(self, now: float) -> dict[str, object]:
        setting = {
            "compute": "execution.wall_timeout_seconds",
            "validation": "validation.timeout_seconds",
            "interactive": "interactive.query_timeout_seconds",
        }[self.phase]
        return {
            "outcome": "ERROR",
            "phase": self.phase,
            "name": self.name,
            "elapsed_seconds": max(0, now - self.started),
            "limit_seconds": self.seconds,
            "setting_source": self.source,
            "setting_key": setting,
            "message": f"{self.phase.capitalize()} timed out for {self.name}. "
            f"Increase {setting} in {self.source}, or set it to 0 to disable it.",
        }
