//! Billing integration tests — DEFERRED to a follow-up tranche.
//!
//! End-to-end Router coverage (spinning up the axum app, hitting
//! `/api/billing`, and asserting the full `BillingState` round-trip) is out of
//! scope for `T-SM-BILLING-auto-detect`: the crate is bin-only, so reaching
//! the Router from an integration test would require extracting a `lib.rs`
//! that re-exports the app builder — a refactor that touches `src/main.rs`
//! (owned by the backend agent) and changes the public surface.
//!
//! Follow-up tranche should: (1) move the Router construction into
//! `src/lib.rs` behind `pub fn build_app(state: AppState) -> Router`,
//! (2) keep `src/main.rs` as a thin shim, (3) add `axum-test` (or
//! `tower::ServiceExt::oneshot`) to dev-deps, (4) cover the four billing
//! modes (forced api, forced subscription, oauth-detected, fallback) plus a
//! mocked `/api/oauth/usage` upstream via mockito.
//!
//! This file is intentionally test-free; it serves as a discoverability
//! marker that the `tests/` directory exists and what belongs in it next.
