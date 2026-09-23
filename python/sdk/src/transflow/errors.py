"""Explicit retry classification for authored source adapters."""


class TransientIOError(Exception):
    """An adapter declares a temporary I/O failure safe to retry on identical inputs.

    Retries require an explicit workspace transient_io policy and attempt budget.
    Adapter-internal request retries must have their own bounded budget. Arbitrary
    exception text is retained only in private logs, never in control diagnostics.
    """
