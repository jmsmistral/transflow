//! Build selection, boundaries, fingerprints, freshness and explanations. No process launching.
/// Pure exact-pin syntax and binding qualification.
pub mod pins;
/// Source refresh and force decisions at a frozen time.
pub mod refresh;
/// Full, selected and between scope over validated candidates.
pub mod scope;
