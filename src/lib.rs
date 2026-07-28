//! probe-leanblueprint: enrich probe-lean Schema 3.0 atoms with Lean blueprint
//! progress metadata.
//!
//! The tool consumes a `probe-lean/extract` atom envelope (the atom "spine")
//! and a blueprint model produced by one of two adapters:
//! - the Verso Blueprint manifest (`versoBlueprint`, machine-readable JSON), or
//! - Patrick Massot's `leanblueprint` (LaTeX/plasTeX), via a bundled headless
//!   emitter that reuses leanblueprint's own parser.
//!
//! It joins blueprint nodes to atoms by Lean declaration name, attaches
//! blueprint extension fields (statement/proof status, uses, group, discussion),
//! and synthesizes atoms for nodes that have no owned real atom — "planned"
//! nodes with no Lean binding, "decl-missing" nodes whose bound decls are all
//! absent, and "shadow" nodes that lost a same-decl collision — so the extract
//! stays node-complete. It then re-emits a `probe-leanblueprint/extract` Schema
//! 3.0 envelope plus a `probe-leanblueprint/summary` sidecar that aggregates
//! progress over the blueprint nodes (overall, per-kind, and per-chapter counts).

pub mod adapters;
pub mod emit;
pub mod emitter;
pub mod enrich;
pub mod error;
pub mod model;

/// Atom key prefix used by probe-lean (`probe:<canonical>`). Shared across the
/// ecosystem (cf. `probe-aeneas`'s `PROBE_PREFIX`).
pub const PROBE_PREFIX: &str = "probe:";
