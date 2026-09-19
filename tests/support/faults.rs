//! One-shot labelled barriers, synchronized by channels rather than real sleeps.

use std::{io, sync::mpsc, time::Duration};

pub const WATCHDOG: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    BeforeStagingWrite,
    AfterArtifactSync,
    BeforeHeadCommit,
    AfterHeadCommit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Continue,
    Cancel,
    NoSpace,
    CoordinatorLost,
}

impl Decision {
    pub fn result(self, boundary: Boundary) -> io::Result<()> {
        let kind = match self {
            Self::Continue => return Ok(()),
            Self::Cancel => io::ErrorKind::Interrupted,
            Self::NoSpace => return Err(io::Error::from_raw_os_error(28)),
            Self::CoordinatorLost => io::ErrorKind::BrokenPipe,
        };
        Err(io::Error::new(
            kind,
            format!("Injected {self:?} at {boundary:?}"),
        ))
    }
}

pub trait Checkpoint {
    fn reach(&mut self, boundary: Boundary) -> io::Result<()>;
}

pub struct Barrier {
    boundary: Boundary,
    arrived: Option<mpsc::SyncSender<Boundary>>,
    decision: mpsc::Receiver<Decision>,
}

pub struct Controller {
    arrived: mpsc::Receiver<Boundary>,
    decision: mpsc::SyncSender<Decision>,
}

pub fn barrier(boundary: Boundary) -> (Barrier, Controller) {
    let (arrived_tx, arrived_rx) = mpsc::sync_channel(1);
    let (decision_tx, decision_rx) = mpsc::sync_channel(1);
    (
        Barrier {
            boundary,
            arrived: Some(arrived_tx),
            decision: decision_rx,
        },
        Controller {
            arrived: arrived_rx,
            decision: decision_tx,
        },
    )
}

impl Checkpoint for Barrier {
    fn reach(&mut self, boundary: Boundary) -> io::Result<()> {
        if boundary != self.boundary {
            return Ok(());
        }
        let sender = self
            .arrived
            .take()
            .ok_or_else(|| io::Error::other("Barrier reached twice"))?;
        sender
            .send(boundary)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Barrier controller lost"))?;
        let decision = self.decision.recv_timeout(WATCHDOG).map_err(|error| {
            let kind = if error == mpsc::RecvTimeoutError::Timeout {
                io::ErrorKind::TimedOut
            } else {
                io::ErrorKind::BrokenPipe
            };
            io::Error::new(kind, format!("No decision at {boundary:?}: {error}"))
        })?;
        decision.result(boundary)
    }
}

impl Controller {
    pub fn wait(&self) -> io::Result<Boundary> {
        self.arrived
            .recv_timeout(WATCHDOG)
            .map_err(|error| io::Error::other(format!("Barrier was not reached: {error}")))
    }
    pub fn release(self, decision: Decision) -> io::Result<()> {
        self.decision
            .send(decision)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Barrier worker lost"))
    }
}
