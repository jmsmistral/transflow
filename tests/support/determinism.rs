//! Deterministic boundary providers. Never use these for production identifiers or secrets.

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

pub trait Clock {
    fn now(&self) -> io::Result<Duration>;
}

#[derive(Clone, Default)]
pub struct VirtualClock(Arc<Mutex<Duration>>);

impl Clock for VirtualClock {
    fn now(&self) -> io::Result<Duration> {
        self.0
            .lock()
            .map(|time| *time)
            .map_err(|_| io::Error::other("Virtual clock poisoned"))
    }
}

impl VirtualClock {
    pub fn advance(&self, elapsed: Duration) -> io::Result<()> {
        let mut time = self
            .0
            .lock()
            .map_err(|_| io::Error::other("Virtual clock poisoned"))?;
        *time = time
            .checked_add(elapsed)
            .ok_or_else(|| io::Error::other("Virtual clock overflow"))?;
        Ok(())
    }
}

pub trait IdSource {
    fn next_id(&mut self) -> io::Result<u128>;
}

pub struct SequentialIds(u128);

impl SequentialIds {
    pub fn starting_at(first: u128) -> Self {
        Self(first)
    }
}

impl IdSource for SequentialIds {
    fn next_id(&mut self) -> io::Result<u128> {
        let current = self.0;
        self.0 = current
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Test ID sequence exhausted"))?;
        Ok(current)
    }
}

/// SplitMix64, with explicit wrapping arithmetic for reproducible synthetic data.
/// This provider is not cryptographic and does not model production UUIDs.
pub struct SeededRandom(u64);

impl SeededRandom {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }
}
