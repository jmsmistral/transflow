//! Scoped provider protection. Renewal outlives hashing, worker draining and cancellation.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tf_exec::supervisor::Cancellation;
use tf_store::{
    artifacts::{ArtifactStore, VerifiedArtifact},
    planning::DraftPlan,
};

pub(super) struct Ticket {
    root: PathBuf,
    workspace: WorkspaceId,
    lease: String,
    operation: String,
    fence: i64,
    release: bool,
    confirmed: Instant,
}
impl Ticket {
    pub(super) fn resolve(
        root: PathBuf,
        workspace: WorkspaceId,
        binding: Selection,
        pin: Option<VersionId>,
        lease: String,
        operation: String,
    ) -> Result<(Self, Value), Error> {
        // Even an interrupted response may have allocated a lease; best-effort release on error.
        let ticket = Self {
            root,
            workspace,
            lease,
            operation,
            fence: 0,
            release: true,
            confirmed: Instant::now(),
        };
        let response = provider::call(
            &ticket.root,
            workspace,
            Request::ResolveRead {
                binding: Box::new(binding),
                pin: pin.map(|v| v.to_string()),
                lease: ticket.lease.clone(),
                operation: ticket.operation.clone(),
            },
        )?;
        Ok((ticket, response))
    }
    fn renew(&mut self) -> Result<(), Error> {
        let confirmed = Instant::now();
        let response = provider::call(
            &self.root,
            self.workspace,
            Request::Renew {
                lease: self.lease.clone(),
                operation: self.operation.clone(),
                fence: self.fence,
            },
        )?;
        self.fence = response["fence"]
            .as_i64()
            .ok_or_else(|| failure("Missing provider read fence"))?;
        self.confirmed = confirmed;
        Ok(())
    }
    pub(super) fn id(&self) -> &str {
        &self.lease
    }
    pub(super) fn retain_for_draft(mut self) {
        self.release = false;
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.release {
            // If the provider is unreachable, the finite expiry is the crash cleanup backstop.
            let _ = provider::call(
                &self.root,
                self.workspace,
                Request::Release {
                    lease: self.lease.clone(),
                    operation: self.operation.clone(),
                    fence: self.fence,
                },
            );
        }
    }
}
#[derive(Default)]
struct State {
    tickets: Vec<Ticket>,
    error: Option<String>,
}
/// One renewal thread per build, independent of the number of external aliases.
/// Declare outside the worker thread scope so its drop follows all worker joins.
pub(crate) struct Reads {
    state: Arc<Mutex<State>>,
    stop: mpsc::SyncSender<()>,
    thread: Option<thread::JoinHandle<()>>,
    entries: BTreeMap<(String, String), (PathBuf, Value)>,
}
impl Reads {
    pub(crate) fn open(
        workspace: &Workspace,
        plan: &DraftPlan,
        cancel: Cancellation,
        verify: bool,
    ) -> Result<Self, Error> {
        let state = Arc::new(Mutex::new(State::default()));
        let (stop, rx) = mpsc::sync_channel(1);
        let shared = state.clone();
        let thread = thread::spawn(move || {
            while matches!(
                rx.recv_timeout(Duration::from_secs(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let Ok(mut s) = shared.lock() else {
                    cancel.cancel();
                    return;
                };
                for ticket in &mut s.tickets {
                    if let Err(e) = ticket.renew() {
                        s.error = Some(format!("Provider read lease renewal failed: {e}"));
                        cancel.cancel();
                        return;
                    }
                }
            }
        });
        let mut reads = Self {
            state,
            stop,
            thread: Some(thread),
            entries: BTreeMap::new(),
        };
        for read in &plan.reads {
            if read.provenance["workspace"] == plan.workspace {
                continue;
            }
            let (ticket, metadata) = acquire(workspace, read, &plan.id)?;
            reads.entries.insert(
                (read.consumer.clone(), read.alias.clone()),
                (ticket.root.clone(), metadata),
            );
            reads.state.lock().map_err(failure)?.tickets.push(ticket);
            if verify {
                reads.verify(&read.consumer, &read.alias)?;
            }
            reads.check()?;
        }
        Ok(reads)
    }
    pub(crate) fn check(&self) -> Result<(), Error> {
        let state = self.state.lock().map_err(failure)?;
        if let Some(error) = &state.error {
            return Err(failure(error));
        }
        // Conservative monotonic deadline detects suspended/delayed renewal threads too.
        if state
            .tickets
            .iter()
            .any(|t| t.confirmed.elapsed() >= Duration::from_secs(840))
        {
            return Err(failure(
                "Provider read protection is no longer confirmed; retry with the provider available",
            ));
        }
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            return Err(failure("Provider read renewal stopped"));
        }
        Ok(())
    }
    pub(crate) fn verify(&self, consumer: &str, alias: &str) -> Result<VerifiedArtifact, Error> {
        self.check()?;
        let (root, metadata) = self
            .entries
            .get(&(consumer.into(), alias.into()))
            .ok_or_else(|| failure("Missing protected provider input"))?;
        let artifact = ArtifactStore::open(root)
            .map_err(failure)?
            .verify(digest(metadata)?)
            .map_err(failure)?;
        if artifact.manifest() != &metadata["manifest"] {
            return Err(failure("Provider bytes differ from selected manifest"));
        }
        self.check()?;
        Ok(artifact)
    }
    /// Retain safe metadata, detecting a provider clone with conflicting immutable provenance.
    pub(crate) async fn record(&self, store: &mut Store) -> Result<(), Error> {
        for (_, metadata) in self.entries.values() {
            let text = |k: &str| {
                metadata[k]
                    .as_str()
                    .ok_or_else(|| failure("Missing provider identity"))
            };
            store
                .record_foreign_metadata(
                    DatasetKey::new(
                        text("workspace")?.parse().map_err(failure)?,
                        text("dataset")?.parse().map_err(failure)?,
                    ),
                    text("version")?.parse().map_err(failure)?,
                    metadata,
                )
                .await
                .map_err(failure)?;
        }
        Ok(())
    }
}
impl Drop for Reads {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Drop tickets after the renewal thread exits: their final fencing generations release.
    }
}
