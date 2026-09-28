// ELK's native worker installs its own message handler. Do not construct the
// bundled in-process ELK adapter inside a worker: it detects self and cannot
// provide its emulated Worker constructor in that environment.
import "elkjs/lib/elk-worker.min.js";
