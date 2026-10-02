//! Deterministic bounded tree selection, expiry and evidence coalescing.
#![allow(
    clippy::unwrap_used,
    reason = "Successful synthetic selection assertions"
)]
use tf_domain::schedule::{Error, Expression as E, Token, evaluate};
fn token<'a>(id: &'a str, leaf: &'a str, time: i64, sequence: u64) -> Token<'a> {
    Token {
        id,
        leaf,
        at_us: time,
        sequence,
        expires_at_us: None,
    }
}
#[test]
fn and_requires_each_child_and_uses_latest_ready_time() {
    let e = E::And(vec![E::Leaf("a"), E::Leaf("b")]);
    let a = token("a1", "a", 10, 1);
    assert_eq!(evaluate(&e, std::slice::from_ref(&a), 100), Ok(None));
    let result = evaluate(&e, &[a, token("b1", "b", 20, 2)], 100)
        .unwrap()
        .unwrap();
    assert_eq!(result.ready_at_us, 20);
    assert_eq!(result.selected, vec![0, 1]);
}
#[test]
fn nested_or_chooses_earliest_ready_with_authored_ties() {
    let e = E::Or(vec![E::And(vec![E::Leaf("a"), E::Leaf("b")]), E::Leaf("c")]);
    let tokens = vec![
        token("a1", "a", 10, 1),
        token("b1", "b", 20, 2),
        token("c1", "c", 20, 3),
    ];
    assert_eq!(
        evaluate(&e, &tokens, 100).unwrap().unwrap().selected,
        vec![0, 1]
    );
    let mut earlier = tokens.clone();
    earlier[2].at_us = 19;
    assert_eq!(
        evaluate(&e, &earlier, 100).unwrap().unwrap().selected,
        vec![2]
    );
}
#[test]
fn coalescing_only_consumes_chosen_leaf_evidence() {
    let e = E::Or(vec![E::Leaf("a"), E::Leaf("b")]);
    let tokens = vec![
        token("a1", "a", 10, 1),
        token("a2", "a", 20, 2),
        token("b1", "b", 30, 3),
    ];
    let result = evaluate(&e, &tokens, 100).unwrap().unwrap();
    assert_eq!(result.selected, vec![1]);
    assert_eq!(result.coalesced, vec![0]);
    assert_eq!(
        evaluate(&e, &tokens[2..], 100).unwrap().unwrap().selected,
        vec![0]
    );
}
#[test]
fn original_time_precedes_delivery_order_and_equal_times_are_stable() {
    let e = E::Leaf("a");
    let tokens = vec![
        token("new", "a", 30, 1),
        token("old", "a", 20, 2),
        token("tie-a", "a", 30, 3),
        token("tie-b", "a", 30, 3),
    ];
    let result = evaluate(&e, &tokens, 100).unwrap().unwrap();
    assert_eq!(result.selected, vec![3]);
    let reversed: Vec<_> = tokens.into_iter().rev().collect();
    assert_eq!(
        evaluate(&e, &reversed, 100).unwrap().unwrap().selected,
        vec![0]
    );
}
#[test]
fn exclusive_expiry_and_future_tokens_do_not_satisfy_leaves() {
    let e = E::Leaf("a");
    let mut expired = token("expired", "a", 10, 1);
    expired.expires_at_us = Some(20);
    let tokens = vec![expired, token("future", "a", 21, 2)];
    assert_eq!(evaluate(&e, &tokens, 20), Ok(None));
    assert_eq!(
        evaluate(&e, &tokens, 19).unwrap().unwrap().selected,
        vec![0]
    );
    assert_eq!(
        evaluate(&e, &tokens, 21).unwrap().unwrap().selected,
        vec![1]
    );
}
#[test]
fn manual_and_tree_bounds_are_explicit() {
    assert_eq!(evaluate(&E::Manual, &[], 0), Ok(None));
    for e in [
        E::And(vec![]),
        E::Or(vec![E::Leaf("a")]),
        E::And(vec![E::Manual, E::Leaf("a")]),
        E::And(vec![E::Leaf("a"), E::Leaf("a")]),
    ] {
        assert_eq!(evaluate(&e, &[], 0), Err(Error::Tree));
    }
    let mut e = E::Leaf("a");
    for _ in 0..8 {
        e = E::Or(vec![e, E::Leaf("b")]);
    }
    assert_eq!(evaluate(&e, &[], 0), Err(Error::Tree));
    let names: Vec<_> = (0..65).map(|i| format!("l{i}")).collect();
    let too_wide = E::Or(names.iter().map(|s| E::Leaf(s)).collect());
    assert_eq!(evaluate(&too_wide, &[], 0), Err(Error::Tree));
}
#[test]
fn invalid_tokens_are_rejected_without_partial_selection() {
    let e = E::Leaf("a");
    assert_eq!(
        evaluate(
            &e,
            &[token("same", "a", 1, 1), token("same", "a", 2, 2)],
            10
        ),
        Err(Error::Tokens)
    );
    assert_eq!(
        evaluate(&e, &[token("unknown", "other", 1, 1)], 10),
        Err(Error::Tokens)
    );
    assert_eq!(
        evaluate(&e, &[token("negative", "a", -1, 1)], 10),
        Err(Error::Tokens)
    );
    assert_eq!(evaluate(&e, &[], -1), Err(Error::Tokens));
    let names: Vec<_> = (0..4097).map(|i| format!("t{i}")).collect();
    let tokens: Vec<_> = names.iter().map(|s| token(s, "a", 1, 1)).collect();
    assert_eq!(evaluate(&e, &tokens, 10), Err(Error::Tokens));
}
