// AGENT COMMUNICATION CHANNEL — Spec 7 v1.1
//
// Per Spec 7 §1.2: "Agents do not send messages to each other. They
// write nodes that other agents observe." This module implements the
// `hotash:comms` region's message and thread types as fabric node
// content types — `Fabric::create()` materializes them with content
// fingerprinting, voice prints, and causal positioning.
//
// Step 1 scope (this module):
// - CommsMessage / CommsThread structs and their enums
// - Materialization helpers: `to_intent_node` for both
// - Metadata key constants downstream observers (immune system,
//   projection bridge) parse to recover structured fields
//
// Steps 2-10 (future):
// - Step 2: thread edge wiring + traversal
// - Step 3: agent subscription dispatch + filtering by mentions
// - Step 4: HandoffContext predicate verification
// - Step 5: DecisionProposal → semantic checkout + conflict detection
// - Step 6: decision-provenance tracing (Cohen I.1 fold)
// - Step 7: OpacityObserver cell-agent specialization
// - Step 8: OperatorIntent companion node for Jeremy-originated messages
// - Step 9: comms_degraded fallback + replay
// - Step 10: simplest-viable projection (log file / Slack webhook)

pub mod handoff;
pub mod message;
pub mod observe;
pub mod thread;

pub use handoff::{find_lineage_by_fingerprint, CheckOutcome};
pub use message::{
    CommsMessage, DecisionProposal, HandoffContext, MessageContent, MessageIntent,
    Sensitivity, SuccessCheck, Urgency, KIND_COMMS_MESSAGE, META_KIND, META_INTENT,
    META_MENTIONS, META_SENSITIVITY, META_THREAD_FINGERPRINT, META_THREAD_NAMESPACE,
    META_URGENCY,
};
pub use observe::{
    is_comms_message, is_mentioned, message_intent, message_mentions_hex, message_urgency,
};
pub use thread::{CommsThread, ThreadState, KIND_COMMS_THREAD, META_THREAD_PARTICIPANTS,
    META_THREAD_STATE, META_THREAD_STARTED_BY, META_THREAD_TOPIC};
