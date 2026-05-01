// DEBUG SURFACE — Snapshot queries + admin-token primitives (Spec 8 §8.5.3)
//
// Per spec §8.5.3 the fabric exposes a debug surface gated to either
// localhost or an admin token:
//   GET /debug/fabric/state
//   GET /debug/fabric/subscriptions
//   GET /debug/fabric/node/<reference>
//   GET /debug/fabric/decay/last-report      (Step 6 territory)
//   GET /debug/fabric/p53/status             (Step 6 territory)
//
// The HTTP endpoints themselves live in Nabu (where the Axum server
// runs). This module contributes:
//   - Structured snapshot types the endpoints serialize over the wire.
//   - `BridgeFabric::debug_*` accessor methods (in `bridge_fabric.rs`)
//     that produce those snapshots from live state.
//   - Admin-token primitives (Spec 8 §8.5.3 v3.1 fold — Cantrill C.3):
//     short-lived (1 hour, non-renewable), Ed25519-signed by an
//     operator key, no rotation, ephemeral.

use crate::identity::{AgentKeypair, NodeIdentity, VoicePrint};
use crate::signature::LineageId;
use ed25519_dalek::{Signature as Ed25519Signature, Verifier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── Snapshot types ─────────────────────────────────────────────────

/// `GET /debug/fabric/state` — top-level fabric summary.
#[derive(Debug, Clone)]
pub struct FabricStateSnapshot {
    pub node_count: usize,
    pub edge_count: usize,
    pub region_count: usize,
    pub subscription_count: usize,
    pub genesis_present: bool,
    pub training_complete: Option<bool>,
    pub current_lamport: u64,
}

/// `GET /debug/fabric/node/<reference>` — full per-node detail.
#[derive(Debug, Clone)]
pub struct NodeDebugDetail {
    pub identity: NodeIdentity,
    pub edit_mode: Option<crate::identity::EditMode>,
    pub want: String,
    pub constraint_count: usize,
    pub edges_out: usize,
    pub edges_in: usize,
    pub quarantine_state: NodeQuarantineLabel,
    pub has_node_signature: bool,
    pub version: u64,
}

/// String label for a `NodeQuarantineState` — flattened for the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeQuarantineLabel {
    Normal,
    Quarantined,
    Confirmed,
    Reversed,
    Dissolved,
}

impl NodeQuarantineLabel {
    pub fn from_state(state: &crate::identity::NodeQuarantineState) -> Self {
        match state {
            crate::identity::NodeQuarantineState::Normal => NodeQuarantineLabel::Normal,
            crate::identity::NodeQuarantineState::Quarantined { .. } => {
                NodeQuarantineLabel::Quarantined
            }
            crate::identity::NodeQuarantineState::Confirmed { .. } => {
                NodeQuarantineLabel::Confirmed
            }
            crate::identity::NodeQuarantineState::Reversed { .. } => {
                NodeQuarantineLabel::Reversed
            }
            crate::identity::NodeQuarantineState::Dissolved => NodeQuarantineLabel::Dissolved,
        }
    }
}

// ── Admin tokens (Spec 8 §8.5.3 — Cantrill C.3 fold) ───────────────

/// Default token lifetime per the spec — 1 hour, non-renewable.
pub const DEBUG_TOKEN_DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

/// Default scope — "debug" — covers all `/debug/*` endpoints.
pub const DEBUG_TOKEN_DEFAULT_SCOPE: &str = "debug";

/// A short-lived signed claim authorizing access to debug endpoints.
///
/// Per Spec 8 §8.5.3 v3.1 fold: "Generation:
/// `fabric.issue_debug_token(operator_key)` where `operator_key` is
/// Jeremy's bootstrap key or a designated operator key. The token is
/// a signed claim `{scope, issued_at, expires_at}`. Verification: the
/// debug endpoint checks the token signature against the known
/// operator pubkey and confirms the timestamp is within bounds.
/// Compromised token impact: 1 hour maximum. No rotation needed —
/// tokens are ephemeral."
#[derive(Debug, Clone)]
pub struct DebugToken {
    pub issuer: VoicePrint,
    /// Wall-clock nanoseconds since UNIX epoch when the token was issued.
    pub issued_at_ns: i128,
    /// Wall-clock nanoseconds since UNIX epoch at which the token expires.
    pub expires_at_ns: i128,
    /// Free-form scope string. v1 uses `"debug"`.
    pub scope: String,
    pub signature: Ed25519Signature,
}

/// Errors emitted by `DebugToken::verify`.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugTokenError {
    /// Token's `expires_at_ns` has passed.
    Expired,
    /// `issued_at_ns` is in the future or `expires_at_ns <= issued_at_ns`.
    InvalidTimestamps,
    /// Issuer claim doesn't match the expected operator key.
    UnknownIssuer,
    /// Ed25519 verification failed.
    BadSignature,
    /// Issuer voice print is not a valid Ed25519 public key.
    MalformedIssuer,
    /// Scope claim doesn't match what the verifier expects.
    ScopeMismatch,
}

impl std::fmt::Display for DebugTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugTokenError::Expired => write!(f, "debug token expired"),
            DebugTokenError::InvalidTimestamps => write!(f, "debug token timestamps invalid"),
            DebugTokenError::UnknownIssuer => write!(f, "debug token issuer not authorized"),
            DebugTokenError::BadSignature => write!(f, "debug token signature failed verification"),
            DebugTokenError::MalformedIssuer => write!(f, "debug token issuer is not a valid Ed25519 key"),
            DebugTokenError::ScopeMismatch => write!(f, "debug token scope mismatch"),
        }
    }
}

impl std::error::Error for DebugTokenError {}

impl DebugToken {
    /// Issue a new token. `lifetime` is added to the wall clock to set
    /// `expires_at`. v1 default: 1 hour.
    pub fn issue(operator: &AgentKeypair, scope: impl Into<String>, lifetime: Duration) -> Self {
        let issued_at_ns = unix_now_ns();
        let expires_at_ns = issued_at_ns + lifetime.as_nanos() as i128;
        let scope: String = scope.into();
        let claims_bytes = canonical_claims_bytes(
            &operator.voice_print(),
            issued_at_ns,
            expires_at_ns,
            &scope,
        );
        let signature = operator.sign(&claims_bytes);
        Self {
            issuer: operator.voice_print(),
            issued_at_ns,
            expires_at_ns,
            scope,
            signature,
        }
    }

    /// Convenience: issue with the spec default lifetime + scope.
    pub fn issue_default(operator: &AgentKeypair) -> Self {
        Self::issue(operator, DEBUG_TOKEN_DEFAULT_SCOPE, DEBUG_TOKEN_DEFAULT_LIFETIME)
    }

    /// Verify the token against the expected operator pubkey and an
    /// expected scope string. Returns `Ok(())` if the token is valid
    /// and within its expiry window.
    pub fn verify(&self, expected_issuer: &VoicePrint, expected_scope: &str) -> Result<(), DebugTokenError> {
        if &self.issuer != expected_issuer {
            return Err(DebugTokenError::UnknownIssuer);
        }
        if self.scope != expected_scope {
            return Err(DebugTokenError::ScopeMismatch);
        }
        if self.expires_at_ns <= self.issued_at_ns {
            return Err(DebugTokenError::InvalidTimestamps);
        }
        let now = unix_now_ns();
        if now >= self.expires_at_ns {
            return Err(DebugTokenError::Expired);
        }
        if now + i128::from(60_000_000_000_i64) < self.issued_at_ns {
            // Issued more than 60 seconds in the future relative to local
            // clock — clock-skew tolerance, but anything wilder is rejected.
            return Err(DebugTokenError::InvalidTimestamps);
        }
        let vk = self
            .issuer
            .to_verifying_key()
            .ok_or(DebugTokenError::MalformedIssuer)?;
        let bytes = canonical_claims_bytes(
            &self.issuer,
            self.issued_at_ns,
            self.expires_at_ns,
            &self.scope,
        );
        vk.verify(&bytes, &self.signature)
            .map_err(|_| DebugTokenError::BadSignature)
    }

    /// How many nanoseconds remain until expiry. Negative if expired.
    pub fn remaining_ns(&self) -> i128 {
        self.expires_at_ns - unix_now_ns()
    }
}

/// Canonical byte representation of the claims that Ed25519 signs.
/// Format: `<voice32><issued_at_ns_be:16><expires_at_ns_be:16><scope_utf8>`.
fn canonical_claims_bytes(
    issuer: &VoicePrint,
    issued_at_ns: i128,
    expires_at_ns: i128,
    scope: &str,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32 + 16 + 16 + scope.len());
    bytes.extend_from_slice(issuer.as_bytes());
    bytes.extend_from_slice(&issued_at_ns.to_be_bytes());
    bytes.extend_from_slice(&expires_at_ns.to_be_bytes());
    bytes.extend_from_slice(scope.as_bytes());
    bytes
}

/// Wall-clock nanoseconds since UNIX epoch as `i128`. Returns 0 if the
/// system clock is somehow before the epoch (shouldn't happen in
/// practice).
fn unix_now_ns() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::generate_agent_keypair;

    #[test]
    fn issue_and_verify_round_trip() {
        let operator = generate_agent_keypair();
        let token = DebugToken::issue_default(&operator);
        assert!(token.verify(&operator.voice_print(), DEBUG_TOKEN_DEFAULT_SCOPE).is_ok());
    }

    #[test]
    fn token_lifetime_default_is_one_hour() {
        let operator = generate_agent_keypair();
        let token = DebugToken::issue_default(&operator);
        let expected_window: i128 = (DEBUG_TOKEN_DEFAULT_LIFETIME.as_nanos()) as i128;
        let actual_window = token.expires_at_ns - token.issued_at_ns;
        assert_eq!(actual_window, expected_window);
    }

    #[test]
    fn wrong_issuer_rejects() {
        let alice = generate_agent_keypair();
        let mallory = generate_agent_keypair();
        let token = DebugToken::issue_default(&alice);
        let result = token.verify(&mallory.voice_print(), DEBUG_TOKEN_DEFAULT_SCOPE);
        assert_eq!(result.unwrap_err(), DebugTokenError::UnknownIssuer);
    }

    #[test]
    fn tampered_signature_rejects() {
        let operator = generate_agent_keypair();
        let mut token = DebugToken::issue_default(&operator);
        // Bump the expiry — signature no longer matches.
        token.expires_at_ns += 1;
        let result = token.verify(&operator.voice_print(), DEBUG_TOKEN_DEFAULT_SCOPE);
        assert_eq!(result.unwrap_err(), DebugTokenError::BadSignature);
    }

    #[test]
    fn expired_token_rejects() {
        let operator = generate_agent_keypair();
        // Issue a token that expired 1 second ago.
        let issued_at_ns = unix_now_ns() - 2_000_000_000;
        let expires_at_ns = unix_now_ns() - 1_000_000_000;
        let scope = DEBUG_TOKEN_DEFAULT_SCOPE.to_string();
        let claims = canonical_claims_bytes(
            &operator.voice_print(),
            issued_at_ns,
            expires_at_ns,
            &scope,
        );
        let signature = operator.sign(&claims);
        let token = DebugToken {
            issuer: operator.voice_print(),
            issued_at_ns,
            expires_at_ns,
            scope,
            signature,
        };
        let result = token.verify(&operator.voice_print(), DEBUG_TOKEN_DEFAULT_SCOPE);
        assert_eq!(result.unwrap_err(), DebugTokenError::Expired);
    }

    #[test]
    fn scope_mismatch_rejects() {
        let operator = generate_agent_keypair();
        let token = DebugToken::issue(&operator, "metrics", DEBUG_TOKEN_DEFAULT_LIFETIME);
        let result = token.verify(&operator.voice_print(), "debug");
        assert_eq!(result.unwrap_err(), DebugTokenError::ScopeMismatch);
    }

    #[test]
    fn remaining_ns_is_positive_for_fresh_token() {
        let operator = generate_agent_keypair();
        let token = DebugToken::issue_default(&operator);
        assert!(token.remaining_ns() > 0);
    }
}

// Stop the unused `LineageId` import warning by referencing it; the
// debug accessors that consume LineageId live in `bridge_fabric.rs`.
#[allow(dead_code)]
fn _link_lineage_id(_: LineageId) {}
