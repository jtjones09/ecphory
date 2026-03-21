// PERSISTENCE — DURABLE FABRIC STATE
//
// Phase 3a: JSON serialization to file.
// Uses parallel serde mirror types to keep core types serde-free.
//
// Design decisions:
// 1. FabricStore trait for pluggable backends.
// 2. JsonFileStore is the bootstrap implementation.
// 3. format_version enables future schema migration.
// 4. FabricInstant (wall clock) is NOT persisted — loaded nodes are "fresh".
// 5. Lamport clock value IS persisted for causal ordering across sessions.

pub mod serial;

use crate::fabric::Fabric;
use serial::SerialFabric;

/// Errors from persistence operations.
#[derive(Debug)]
pub enum PersistError {
    /// File I/O failure.
    IoError(String),
    /// Serialization to JSON failed.
    SerializationError(String),
    /// Deserialization from JSON failed.
    DeserializationError(String),
    /// Persisted format version doesn't match expected.
    VersionMismatch { expected: u32, found: u32 },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistError::IoError(e) => write!(f, "IO error: {}", e),
            PersistError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            PersistError::DeserializationError(e) => write!(f, "Deserialization error: {}", e),
            PersistError::VersionMismatch { expected, found } => {
                write!(f, "Version mismatch: expected {}, found {}", expected, found)
            }
        }
    }
}

/// Trait for fabric persistence backends.
///
/// Phase 3a: JsonFileStore (JSON to file).
/// Future: Network-based stores, embedded databases, etc.
pub trait FabricStore {
    /// Save the fabric state to durable storage.
    fn save(&self, fabric: &Fabric) -> Result<(), PersistError>;

    /// Load a fabric from durable storage.
    fn load(&self) -> Result<Fabric, PersistError>;

    /// Check if a persisted fabric exists at this location.
    fn exists(&self) -> bool;
}

/// JSON file persistence backend.
///
/// Serializes the entire fabric state to a single JSON file.
/// Pretty-printed for human readability (debugging, inspection).
///
/// Phase 3a bootstrap. Phase 3d may add append-only log or
/// network-backed alternatives.
pub struct JsonFileStore {
    path: String,
}

impl JsonFileStore {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl FabricStore for JsonFileStore {
    fn save(&self, fabric: &Fabric) -> Result<(), PersistError> {
        let serial = SerialFabric::from_fabric(fabric);
        let json = serde_json::to_string_pretty(&serial)
            .map_err(|e| PersistError::SerializationError(e.to_string()))?;
        std::fs::write(&self.path, json)
            .map_err(|e| PersistError::IoError(e.to_string()))?;
        Ok(())
    }

    fn load(&self) -> Result<Fabric, PersistError> {
        let json = std::fs::read_to_string(&self.path)
            .map_err(|e| PersistError::IoError(e.to_string()))?;
        let serial: SerialFabric = serde_json::from_str(&json)
            .map_err(|e| PersistError::DeserializationError(e.to_string()))?;
        serial.into_fabric()
    }

    fn exists(&self) -> bool {
        std::path::Path::new(&self.path).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RelationshipKind;
    use crate::node::IntentNode;
    use std::fs;

    fn temp_path(name: &str) -> String {
        format!("/tmp/intent_node_test_{}.json", name)
    }

    fn cleanup(path: &str) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn json_store_save_creates_file() {
        let path = temp_path("save_creates");
        cleanup(&path);
        let store = JsonFileStore::new(&path);
        let fabric = Fabric::new();
        store.save(&fabric).unwrap();
        assert!(store.exists());
        cleanup(&path);
    }

    #[test]
    fn json_store_load_returns_fabric() {
        let path = temp_path("load_returns");
        cleanup(&path);
        let store = JsonFileStore::new(&path);
        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("test node"));
        store.save(&fabric).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.node_count(), 1);
        cleanup(&path);
    }

    #[test]
    fn json_store_roundtrip_preserves_node_count() {
        let path = temp_path("node_count");
        cleanup(&path);
        let store = JsonFileStore::new(&path);

        let mut fabric = Fabric::new();
        fabric.add_node(IntentNode::new("one"));
        fabric.add_node(IntentNode::new("two"));
        fabric.add_node(IntentNode::new("three"));
        store.save(&fabric).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.node_count(), 3);
        cleanup(&path);
    }

    #[test]
    fn json_store_roundtrip_preserves_edges() {
        let path = temp_path("edges");
        cleanup(&path);
        let store = JsonFileStore::new(&path);

        let mut fabric = Fabric::new();
        let a = fabric.add_node(IntentNode::new("node a"));
        let b = fabric.add_node(IntentNode::new("node b"));
        fabric.add_edge(&a, &b, 0.9, RelationshipKind::DependsOn).unwrap();
        store.save(&fabric).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.edge_count(), 1);
        // Verify the edge target is correct
        let edges = loaded.edges_from(&a);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, b);
        cleanup(&path);
    }

    #[test]
    fn json_store_load_nonexistent_errors() {
        let store = JsonFileStore::new("/tmp/intent_node_test_nonexistent_xyz.json");
        let result = store.load();
        assert!(result.is_err());
    }

    #[test]
    fn json_store_exists_false_when_missing() {
        let store = JsonFileStore::new("/tmp/intent_node_test_missing_xyz.json");
        assert!(!store.exists());
    }
}
