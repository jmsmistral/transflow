//! Bounded, expiring browser grants. Tokens never implement Debug or enter response diagnostics.
use crate::ApiError;
use std::{
    collections::BTreeMap,
    io::Read,
    time::{Duration, Instant},
};

pub(crate) fn secret() -> Result<String, ApiError> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| ApiError::internal())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub(crate) fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |n, (x, y)| n | (x ^ y)) == 0
}
struct Session {
    csrf: String,
    expires: Instant,
}
pub(crate) struct Grants {
    launches: BTreeMap<String, Instant>,
    sessions: BTreeMap<String, Session>,
}
impl Grants {
    pub fn new() -> Self {
        Self {
            launches: BTreeMap::new(),
            sessions: BTreeMap::new(),
        }
    }
    fn prune(&mut self, now: Instant) {
        self.launches.retain(|_, e| *e > now);
        self.sessions.retain(|_, s| s.expires > now);
    }
    pub fn launch(&mut self, now: Instant) -> Result<String, ApiError> {
        self.prune(now);
        if self.launches.len() >= 32 {
            return Err(ApiError::busy());
        }
        let code = secret()?;
        self.launches
            .insert(code.clone(), now + Duration::from_secs(60));
        Ok(code)
    }
    pub fn exchange(&mut self, code: &str, now: Instant) -> Result<(String, String), ApiError> {
        self.prune(now);
        if self.launches.remove(code).is_none() {
            return Err(ApiError::unauthorized());
        }
        if self.sessions.len() >= 128 {
            return Err(ApiError::busy());
        }
        let cookie = secret()?;
        let csrf = secret()?;
        self.sessions.insert(
            cookie.clone(),
            Session {
                csrf: csrf.clone(),
                expires: now + Duration::from_secs(12 * 3600),
            },
        );
        Ok((cookie, csrf))
    }
    pub fn validate(
        &mut self,
        cookie: &str,
        csrf: Option<&str>,
        mutation: bool,
        now: Instant,
    ) -> bool {
        self.prune(now);
        self.sessions
            .get(cookie)
            .is_some_and(|s| !mutation || csrf.is_some_and(|c| same(c, &s.csrf)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_are_single_use_expiring_bounded_and_require_the_session_csrf() -> Result<(), ApiError>
    {
        let mut g = Grants::new();
        let now = Instant::now();
        let code = g.launch(now)?;
        let (cookie, csrf) = g.exchange(&code, now)?;
        assert!(g.exchange(&code, now).is_err());
        assert!(!g.validate(&cookie, None, true, now));
        assert!(!g.validate(&cookie, Some("wrong"), true, now));
        assert!(g.validate(&cookie, Some(&csrf), true, now));
        assert!(!g.validate(
            &cookie,
            Some(&csrf),
            true,
            now + Duration::from_secs(12 * 3600)
        ));
        let expired = g.launch(now)?;
        assert!(g.exchange(&expired, now + Duration::from_secs(60)).is_err());
        for _ in 0..32 {
            g.launch(now)?;
        }
        assert!(g.launch(now).is_err());
        Ok(())
    }
}
