//! Rust bindings for the Lazarus Protocol.
//!
//! The generated contract registry lives here:
//!
//! [`generated_registry`] is the per-method contract manifest (names,
//! versions, JSON Schema fingerprints), regenerated from the
//! TypeScript/Zod source of truth with
//! `pnpm --filter @lazarus/protocol-ts gen:bindings`. Do not edit by hand.
//!
//! Alongside it are hand-written transport-neutral contracts shared by
//! Host, CLI, and Desktop: local token [`auth`], the idempotency-key
//! convention ([`idempotency`]), the cancellation/deadline convention
//! ([`deadline`]), and per-method manifest encoding and version negotiation
//! ([`manifest`]).

/// Contract registry generated from the TypeScript/Zod protocol package.
#[path = "generated_registry.rs"]
pub mod generated_registry;

/// Local token authentication primitives shared by Host, CLI, and Desktop.
pub mod auth;
/// Executable version bridges driven by the generated declarative steps.
pub mod bridges;
/// Transport-neutral cancellation and deadline contract.
pub mod deadline;
/// Idempotency-key contract for write methods (plan section 9.1).
pub mod idempotency;
/// Per-method manifest encoding and version negotiation.
pub mod manifest;
