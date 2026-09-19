"""Fixed synthetic child modes for the T011 POSIX supervision probe."""

import json
import os
import resource
import signal
import subprocess
import sys
from pathlib import Path


def emit(value):
    print(json.dumps(value), flush=True)


def rss_bytes():
    raw = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(raw if sys.platform == "darwin" else raw * 1024)


def main(mode):
    if mode == "stubborn":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        emit({"ready": True, "pid": os.getpid()})
        while True:
            signal.pause()
    elif mode == "leaf":
        emit({"ready": True, "pid": os.getpid()})
        signal.pause()
    elif mode == "tree":
        # The descendant inherits the managed process group. Parent reaps it after TERM.
        with subprocess.Popen(
            [sys.executable, "-I", "-B", str(Path(__file__).resolve()), "leaf"],
            stdout=subprocess.PIPE,
            text=True,
        ) as leaf:
            assert leaf.stdout is not None
            ready = json.loads(leaf.stdout.readline())

            def stop(signum, frame):
                leaf.wait(timeout=5)
                emit({"descendant_reaped": True, "returncode": leaf.returncode})
                raise SystemExit(0)

            signal.signal(signal.SIGTERM, stop)
            emit({"ready": True, "pid": os.getpid(), "descendant": ready["pid"]})
            signal.pause()
    elif mode == "polars":
        import polars as pl

        frame = pl.DataFrame({"x": range(100_000)})
        query = frame.lazy().select(pl.col("x").cast(pl.Float64).sqrt().sum())
        assert query.collect(engine="streaming").height == 1
        emit(
            {
                "ready": True,
                "threads": pl.thread_pool_size(),
                "max_rss_bytes": rss_bytes(),
                "engine": pl.__version__,
                "transflow_memory_budget": None,
            }
        )
        # A real, repeatable compute workload, bounded independently of timeout policy.
        while True:
            query.collect(engine="streaming")
    elif mode == "duckdb":
        import duckdb

        with duckdb.connect(config={"threads": 1, "enable_external_access": False}) as db:
            assert db.execute("SELECT sum(i) FROM range(100000) t(i)").fetchone()[0] == 4999950000
            memory = db.execute("SELECT current_setting('memory_limit')").fetchone()[0]
            emit(
                {
                    "ready": True,
                    "threads": 1,
                    "max_rss_bytes": rss_bytes(),
                    "engine": duckdb.__version__,
                    "engine_memory_limit": memory,
                    "transflow_memory_budget": None,
                }
            )
            while True:
                db.execute("SELECT sum(sqrt(i)) FROM range(1000000) t(i)").fetchone()
    else:
        raise ValueError("Unknown fixed worker mode")


if __name__ == "__main__":
    main(sys.argv[1])
