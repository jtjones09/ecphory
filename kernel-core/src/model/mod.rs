//! The nucleation generative model.
//!
//! Over time this module grows into the unified `GenerativeModel`
//! described in `nisaba/positions/nucleation-architecture.md`: one
//! agent, one model, one inference loop, with regions specialising
//! through experience. The math primitive (`DiscreteModel`) is the
//! foundation every region is built on.
//!
//! ## Status
//!
//! - `discrete::DiscreteModel` — generic per-region POMDP factor (✅ live)
//! - `GenerativeModel` — composed multi-region model (in progress, see Step 2)
//! - `CausalGraph`, `PatternEngine`, `MetaModel` — coming in later steps
//!
//! Per the nucleation handoff: regions for v1 are explicitly named
//! (devices, persistence, operator, meta). Emergent regions via mutual-
//! information clique discovery (Friston review F.8) is v2 work — true
//! emergence is a research contribution, not a v1 feature.

pub mod discrete;

pub use discrete::DiscreteModel;
