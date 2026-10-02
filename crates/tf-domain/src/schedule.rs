//! Pure, bounded trigger evaluation. No storage, clock reads or identity allocation.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Logical structure after the protocol has validated authored leaf semantics.
#[derive(Clone, Debug)]
pub enum Expression<'a> {
    /// Explicit manual schedules never become automatically ready.
    Manual,
    /// Stable event or clock leaf identity.
    Leaf(&'a str),
    /// Require evidence from every child.
    And(Vec<Self>),
    /// Choose the earliest-ready child, preserving authored order on ties.
    Or(Vec<Self>),
}
/// Unconsumed evidence supplied by the owner, independent of arrival order.
#[derive(Clone, Debug)]
pub struct Token<'a> {
    /// Stable token identity, used only as a final deterministic tie breaker.
    pub id: &'a str,
    /// Stable leaf identity.
    pub leaf: &'a str,
    /// Original event/tick time in microseconds.
    pub at_us: i64,
    /// Monotonic event ordering for equal occurrence times.
    pub sequence: u64,
    /// Exclusive expiry; None is an explicitly acknowledged unlimited lifetime.
    pub expires_at_us: Option<i64>,
}
/// Selected evidence indexes into the supplied token slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    /// Latest selected child-ready time for AND; chosen child's time for OR.
    pub ready_at_us: i64,
    /// One latest eligible token per chosen leaf, in authored tree order.
    pub selected: Vec<usize>,
    /// Older eligible tokens for chosen leaves only, in input order.
    pub coalesced: Vec<usize>,
}
/// Invalid input is rejected before recursion or selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Depth, width, manual nesting, empty node or duplicate leaf violation.
    Tree,
    /// Duplicate/unknown token identity, negative time or token budget violation.
    Tokens,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tree => "The trigger tree exceeds its bounds or has invalid leaf identities",
            Self::Tokens => {
                "Trigger evidence exceeds its bounds or contains invalid identities or times"
            }
        })
    }
}
impl std::error::Error for Error {}
fn validate<'a>(
    e: &Expression<'a>,
    depth: usize,
    ids: &mut BTreeSet<&'a str>,
) -> Result<(), Error> {
    if depth > 8 {
        return Err(Error::Tree);
    }
    match e {
        Expression::Manual if depth == 1 => {}
        Expression::Manual => return Err(Error::Tree),
        Expression::Leaf(id) => {
            if id.is_empty()
                || id.len() > 256
                || id.chars().any(char::is_control)
                || !ids.insert(id)
                || ids.len() > 64
            {
                return Err(Error::Tree);
            }
        }
        Expression::And(children) | Expression::Or(children) => {
            if children.len() < 2 || children.len() > 64 {
                return Err(Error::Tree);
            }
            for child in children {
                validate(child, depth + 1, ids)?;
            }
        }
    }
    Ok(())
}
fn choose(
    e: &Expression<'_>,
    latest: &BTreeMap<&str, usize>,
    tokens: &[Token<'_>],
) -> Option<Selection> {
    match e {
        Expression::Manual => None,
        Expression::Leaf(id) => {
            let index = *latest.get(id)?;
            Some(Selection {
                ready_at_us: tokens.get(index)?.at_us,
                selected: vec![index],
                coalesced: vec![],
            })
        }
        Expression::And(children) => {
            let mut result = Selection {
                ready_at_us: 0,
                selected: vec![],
                coalesced: vec![],
            };
            for child in children {
                let next = choose(child, latest, tokens)?;
                result.ready_at_us = result.ready_at_us.max(next.ready_at_us);
                result.selected.extend(next.selected);
            }
            Some(result)
        }
        Expression::Or(children) => {
            let mut result: Option<Selection> = None;
            for child in children {
                if let Some(next) = choose(child, latest, tokens)
                    && result
                        .as_ref()
                        .is_none_or(|old| next.ready_at_us < old.ready_at_us)
                {
                    result = Some(next);
                }
            }
            result
        }
    }
}
/// Latest means greatest original occurrence time, then event sequence, then token ID.
/// Expired/future tokens cannot satisfy a leaf. OR leftovers are never coalesced.
/// Inputs are bounded to depth 8, 64 leaves and 4,096 tokens.
pub fn evaluate(
    e: &Expression<'_>,
    tokens: &[Token<'_>],
    now_us: i64,
) -> Result<Option<Selection>, Error> {
    let mut leaves = BTreeSet::new();
    validate(e, 1, &mut leaves)?;
    if now_us < 0 || tokens.len() > 4096 {
        return Err(Error::Tokens);
    }
    let mut ids = BTreeSet::new();
    let mut latest: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.id.is_empty()
            || !ids.insert(token.id)
            || !leaves.contains(token.leaf)
            || token.at_us < 0
            || token.expires_at_us.is_some_and(|t| t < token.at_us)
        {
            return Err(Error::Tokens);
        }
        if token.at_us > now_us || token.expires_at_us.is_some_and(|t| t <= now_us) {
            continue;
        }
        let newer = latest
            .get(token.leaf)
            .and_then(|i| tokens.get(*i))
            .is_none_or(|old| {
                (token.at_us, token.sequence, token.id) > (old.at_us, old.sequence, old.id)
            });
        if newer {
            latest.insert(token.leaf, index);
        }
    }
    let Some(mut result) = choose(e, &latest, tokens) else {
        return Ok(None);
    };
    let chosen: BTreeSet<_> = result
        .selected
        .iter()
        .filter_map(|i| tokens.get(*i).map(|t| t.leaf))
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if chosen.contains(token.leaf)
            && !result.selected.contains(&index)
            && token.at_us <= now_us
            && token.expires_at_us.is_none_or(|t| t > now_us)
        {
            result.coalesced.push(index);
        }
    }
    Ok(Some(result))
}
