//! Bounded cron/timezone resolution and explicit clock observation. No daemon or build dispatch.
pub mod cron;
pub use cron::{Clock, Error, Fire, SystemClock, prepare, preview};
