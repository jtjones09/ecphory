// TRANSPORT — COMMUNICATION LAYER
//
// Trait for sending gossip messages between replicas.
// Phase 3d: LocalTransport (loopback, single process).
// Phase 4: Network transport (TCP, QUIC, etc.)
//
// Design decisions:
// 1. Synchronous for Phase 3d. Async deferred.
// 2. LocalTransport uses in-memory queue for testing.
// 3. Transport trait is minimal — send and receive.

use std::collections::VecDeque;
use super::gossip::GossipMessage;
use super::vclock::ReplicaId;

/// A message envelope with sender information.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub from: ReplicaId,
    pub to: ReplicaId,
    pub message: GossipMessage,
}

/// Trait for gossip message transport.
pub trait Transport {
    /// Send a message to another replica.
    fn send(&mut self, envelope: Envelope);

    /// Receive pending messages for a specific replica.
    fn receive(&mut self, replica: &ReplicaId) -> Vec<Envelope>;

    /// Check if there are pending messages for a replica.
    fn has_pending(&self, replica: &ReplicaId) -> bool;
}

/// In-process loopback transport for testing.
///
/// Messages are queued in memory. No network involved.
/// Phase 3d only — real transport in Phase 4.
#[derive(Debug)]
pub struct LocalTransport {
    /// Queue of pending messages.
    queue: VecDeque<Envelope>,
}

impl LocalTransport {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Total messages in the queue.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

impl Default for LocalTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for LocalTransport {
    fn send(&mut self, envelope: Envelope) {
        self.queue.push_back(envelope);
    }

    fn receive(&mut self, replica: &ReplicaId) -> Vec<Envelope> {
        let mut received = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(env) = self.queue.pop_front() {
            if &env.to == replica {
                received.push(env);
            } else {
                remaining.push_back(env);
            }
        }

        self.queue = remaining;
        received
    }

    fn has_pending(&self, replica: &ReplicaId) -> bool {
        self.queue.iter().any(|env| &env.to == replica)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_transport_send_receive() {
        let mut transport = LocalTransport::new();
        let sender = ReplicaId::new();
        let receiver = ReplicaId::new();

        let digest = super::super::gossip::Digest { entries: vec![] };
        transport.send(Envelope {
            from: sender.clone(),
            to: receiver.clone(),
            message: GossipMessage::Digest(digest),
        });

        assert!(transport.has_pending(&receiver));
        assert!(!transport.has_pending(&sender));

        let messages = transport.receive(&receiver);
        assert_eq!(messages.len(), 1);
        assert!(!transport.has_pending(&receiver));
    }

    #[test]
    fn local_transport_filters_by_recipient() {
        let mut transport = LocalTransport::new();
        let a = ReplicaId::new();
        let b = ReplicaId::new();
        let c = ReplicaId::new();

        let digest = super::super::gossip::Digest { entries: vec![] };

        // Send to b and c.
        transport.send(Envelope {
            from: a.clone(), to: b.clone(),
            message: GossipMessage::Digest(digest.clone()),
        });
        transport.send(Envelope {
            from: a.clone(), to: c.clone(),
            message: GossipMessage::Digest(digest.clone()),
        });

        // Receive for b only.
        let msgs = transport.receive(&b);
        assert_eq!(msgs.len(), 1);
        assert_eq!(transport.queue_len(), 1); // c's message remains
    }

    #[test]
    fn local_transport_empty_receive() {
        let mut transport = LocalTransport::new();
        let r = ReplicaId::new();
        let msgs = transport.receive(&r);
        assert!(msgs.is_empty());
    }
}
