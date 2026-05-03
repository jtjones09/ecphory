// COMMS PROJECTION BRIDGE — Spec 7 §5 / Step 10
//
// Per Spec 7 §5.2 v1.1 fold (Cantrill C.1 + Isimud IS.2): "signal-cli
// is a Java application with significant dependency overhead… For v1,
// consider a simpler projection first (Slack webhook or log file) with
// Signal as a v1.5 upgrade. The comms substrate is the priority; the
// projection layer is scaffolding that can iterate."
//
// v1 implementation: a log-file/text-sink projection that:
// - Formats each comms message as a single line
// - Honors the urgency → notification mapping from Spec 7 §5.3
// - Honors the Do Not Disturb default (22:00–07:00 local) per
//   Cantrill C.3 + Jeremy J.1 fold: Background / Normal / Prompt are
//   held; Immediate is delivered ONCE without 5-minute repeats
//
// The projection takes the local hour (0..=23) as a parameter so the
// fabric stays timezone-naive — callers (Nabu, nisaba-the-agent)
// inject the local hour their device reports.

use std::sync::{Arc, Mutex};

use crate::comms::message::{Urgency, KIND_COMMS_MESSAGE, META_KIND, META_URGENCY};
use crate::comms::observe::{message_intent, message_urgency};
use crate::node::IntentNode;

pub const DEFAULT_DND_START_HOUR: u8 = 22; // 10 PM
pub const DEFAULT_DND_END_HOUR: u8 = 7; //  7 AM (held messages release at 07:00)

/// Result of attempting a single projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionDecision {
    /// The line was written to the sink.
    Delivered,
    /// Outside scope (not a comms message; not a node we project).
    Skipped,
    /// Held for later release; will surface via `flush(now)` after the
    /// DND window ends.
    Held,
}

/// Where formatted lines go. v1 supports an in-memory `Vec` (tests +
/// development) or any closure (production: append to file, POST to
/// Slack webhook, etc.).
pub type Sink = Arc<dyn Fn(String) + Send + Sync>;

/// A buffered, DND-aware projection of the comms region.
pub struct LogProjection {
    sink: Sink,
    /// DND window. Inclusive of `start_hour`, exclusive of `end_hour`.
    /// If `start_hour > end_hour` the window wraps midnight (the
    /// default 22→7 case).
    dnd_start_hour: u8,
    dnd_end_hour: u8,
    /// Messages held during DND. Released by `flush`.
    held: Mutex<Vec<HeldMessage>>,
}

#[derive(Debug, Clone)]
struct HeldMessage {
    line: String,
    /// Retained for future v1.5 work — separating Prompt-vs-Normal
    /// holds, batched daily summaries for Background, etc.
    #[allow(dead_code)]
    urgency: Urgency,
}

impl LogProjection {
    pub fn new(sink: Sink) -> Self {
        Self {
            sink,
            dnd_start_hour: DEFAULT_DND_START_HOUR,
            dnd_end_hour: DEFAULT_DND_END_HOUR,
            held: Mutex::new(Vec::new()),
        }
    }

    pub fn with_dnd_window(mut self, start_hour: u8, end_hour: u8) -> Self {
        self.dnd_start_hour = start_hour;
        self.dnd_end_hour = end_hour;
        self
    }

    /// Project a single fabric node. Returns the per-node decision
    /// after taking DND into account.
    pub fn project(&self, node: &IntentNode, current_hour: u8) -> ProjectionDecision {
        let is_comms = node
            .metadata
            .get(META_KIND)
            .map(|v| v.as_str_repr() == KIND_COMMS_MESSAGE)
            .unwrap_or(false);
        if !is_comms {
            return ProjectionDecision::Skipped;
        }

        let urgency = message_urgency(node).unwrap_or(Urgency::Normal);
        let line = format_line(node, urgency);
        let in_dnd = is_in_dnd_window(current_hour, self.dnd_start_hour, self.dnd_end_hour);

        if in_dnd && urgency != Urgency::Immediate {
            self.held
                .lock()
                .unwrap()
                .push(HeldMessage { line, urgency });
            return ProjectionDecision::Held;
        }

        (self.sink)(line);
        ProjectionDecision::Delivered
    }

    /// Release any held messages now that DND has ended (or because
    /// the caller forces a flush). Returns the count of messages
    /// flushed to the sink.
    pub fn flush(&self, current_hour: u8) -> usize {
        if is_in_dnd_window(current_hour, self.dnd_start_hour, self.dnd_end_hour) {
            return 0;
        }
        let mut held = self.held.lock().unwrap();
        let mut flushed = 0;
        for msg in held.drain(..) {
            (self.sink)(msg.line);
            flushed += 1;
        }
        flushed
    }

    /// Number of messages currently held pending DND release.
    pub fn pending_count(&self) -> usize {
        self.held.lock().unwrap().len()
    }
}

fn is_in_dnd_window(current_hour: u8, start_hour: u8, end_hour: u8) -> bool {
    if start_hour <= end_hour {
        // Same-day window, e.g., 09..=17.
        current_hour >= start_hour && current_hour < end_hour
    } else {
        // Wraps midnight, e.g., 22..=07.
        current_hour >= start_hour || current_hour < end_hour
    }
}

fn format_line(node: &IntentNode, urgency: Urgency) -> String {
    let intent = message_intent(node)
        .map(|i| i.label())
        .unwrap_or("Inform");
    let voice = node
        .creator_voice
        .as_ref()
        .map(|v| {
            let hex = v.to_hex();
            format!("{}…{}", &hex[..6], &hex[hex.len() - 6..])
        })
        .unwrap_or_else(|| "<unsigned>".into());
    format!(
        "[{} | {} | {}] {}",
        urgency.label(),
        intent,
        voice,
        node.want.description
    )
}

/// Convenience: an in-memory `Vec<String>` sink for tests +
/// development. Returns the projection's sink closure paired with the
/// shared buffer the caller can read.
pub fn vec_sink() -> (Sink, Arc<Mutex<Vec<String>>>) {
    let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let buf_for_sink = Arc::clone(&buf);
    let sink: Sink = Arc::new(move |line: String| {
        buf_for_sink.lock().unwrap().push(line);
    });
    (sink, buf)
}

/// Note: subscribers receive nodes with the `__comms_urgency__`
/// metadata key, even when `Urgency::Immediate` is rendered. Used by
/// callers that want to read the urgency without parsing the level.
pub const _META_URGENCY: &str = META_URGENCY;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms::{
        CommsMessage, MessageContent, MessageIntent, Sensitivity, Urgency as MsgUrgency,
    };
    use crate::identity::generate_agent_keypair;

    fn build_message(urgency: MsgUrgency) -> IntentNode {
        let agent = generate_agent_keypair();
        let msg = CommsMessage {
            content: MessageContent::Text("hello".into()),
            thread: None,
            mentions: vec![],
            intent: MessageIntent::Inform,
            urgency,
            sensitivity: Sensitivity::Normal,
            references: vec![],
        };
        msg.to_intent_node(agent.voice_print())
    }

    #[test]
    fn dnd_window_calculation_handles_midnight_wrap() {
        // 22..=07 wraps midnight.
        assert!(is_in_dnd_window(22, 22, 7));
        assert!(is_in_dnd_window(2, 22, 7));
        assert!(is_in_dnd_window(6, 22, 7));
        assert!(!is_in_dnd_window(7, 22, 7));
        assert!(!is_in_dnd_window(12, 22, 7));
    }

    #[test]
    fn outside_dnd_normal_message_delivers_immediately() {
        let (sink, buf) = vec_sink();
        let proj = LogProjection::new(sink);
        let node = build_message(MsgUrgency::Normal);
        let decision = proj.project(&node, 14);
        assert_eq!(decision, ProjectionDecision::Delivered);
        let buf = buf.lock().unwrap();
        assert_eq!(buf.len(), 1);
        assert!(buf[0].contains("Normal"));
        assert!(buf[0].contains("hello"));
    }

    #[test]
    fn inside_dnd_non_immediate_held_until_flush() {
        let (sink, buf) = vec_sink();
        let proj = LogProjection::new(sink);
        let normal = build_message(MsgUrgency::Normal);
        let prompt = build_message(MsgUrgency::Prompt);
        let bg = build_message(MsgUrgency::Background);
        // 02:00 — inside default DND.
        assert_eq!(proj.project(&normal, 2), ProjectionDecision::Held);
        assert_eq!(proj.project(&prompt, 2), ProjectionDecision::Held);
        assert_eq!(proj.project(&bg, 2), ProjectionDecision::Held);
        assert_eq!(proj.pending_count(), 3);
        assert_eq!(buf.lock().unwrap().len(), 0);

        // 07:00 — DND released.
        let flushed = proj.flush(7);
        assert_eq!(flushed, 3);
        assert_eq!(proj.pending_count(), 0);
        assert_eq!(buf.lock().unwrap().len(), 3);
    }

    #[test]
    fn immediate_urgency_delivers_during_dnd() {
        let (sink, buf) = vec_sink();
        let proj = LogProjection::new(sink);
        let immediate = build_message(MsgUrgency::Immediate);
        // 03:00 — deep DND, but Immediate goes through.
        assert_eq!(proj.project(&immediate, 3), ProjectionDecision::Delivered);
        assert_eq!(proj.pending_count(), 0);
        let buf = buf.lock().unwrap();
        assert_eq!(buf.len(), 1);
        assert!(buf[0].contains("Immediate"));
    }

    #[test]
    fn non_comms_nodes_are_skipped() {
        let (sink, buf) = vec_sink();
        let proj = LogProjection::new(sink);
        let plain = IntentNode::new("not a comms message");
        assert_eq!(proj.project(&plain, 14), ProjectionDecision::Skipped);
        assert_eq!(buf.lock().unwrap().len(), 0);
    }
}
