// INTENT CLI — Bootstrap tool for the Ecphory Fabric
//
// Commands:
//   intent fabric add       --want "..." [--meta "key=value"]... [--project name]
//   intent fabric list      [--project name]
//   intent fabric search    [--query "..."] [--where "key=value AND ..."] [--project name]
//   intent fabric aggregate --field F --op OP [--where "..."] [--group-by key] [--project name]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process;

use ecphory::node::{IntentNode, MetadataValue};
use ecphory::persist::{FabricStore, JsonFileStore};
use ecphory::fabric::Fabric;
use ecphory::embedding::bow::BagOfWordsEmbedder;
use ecphory::signature::LineageId;

// ═══════════════════════════════════════════════
//  Predicate Parser
// ═══════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum CmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

#[derive(Debug, Clone)]
struct Predicate {
    key: String,
    op: CmpOp,
    value: String,
}

fn parse_predicates(where_clause: &str) -> Vec<Predicate> {
    let parts: Vec<&str> = where_clause.split(" AND ").collect();
    let mut predicates = Vec::new();

    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(pred) = parse_single_predicate(part) {
            predicates.push(pred);
        } else {
            eprintln!("Warning: could not parse predicate '{}'", part);
        }
    }
    predicates
}

fn parse_single_predicate(s: &str) -> Option<Predicate> {
    // Try operators in order of length (longest first to avoid partial matches)
    for (op_str, op) in &[
        ("!=", CmpOp::Ne),
        (">=", CmpOp::Ge),
        ("<=", CmpOp::Le),
        (">", CmpOp::Gt),
        ("<", CmpOp::Lt),
        ("=", CmpOp::Eq),
    ] {
        if let Some(pos) = s.find(op_str) {
            let key = s[..pos].trim().to_string();
            let value = s[pos + op_str.len()..].trim().to_string();
            if !key.is_empty() && !value.is_empty() {
                return Some(Predicate { key, op: op.clone(), value });
            }
        }
    }
    None
}

fn predicate_matches(metadata: &HashMap<String, MetadataValue>, pred: &Predicate) -> bool {
    let meta_val = match metadata.get(&pred.key) {
        Some(v) => v,
        None => return false, // Missing key = no match
    };

    let pred_parsed = MetadataValue::parse(&pred.value);

    match (&pred.op, meta_val, &pred_parsed) {
        // String comparisons
        (CmpOp::Eq, MetadataValue::String(a), MetadataValue::String(b)) => a == b,
        (CmpOp::Ne, MetadataValue::String(a), MetadataValue::String(b)) => a != b,

        // Bool comparisons
        (CmpOp::Eq, MetadataValue::Bool(a), MetadataValue::Bool(b)) => a == b,
        (CmpOp::Ne, MetadataValue::Bool(a), MetadataValue::Bool(b)) => a != b,

        // Numeric comparisons — compare as f64
        (op, _, _) => {
            match (meta_val.as_f64(), pred_parsed.as_f64()) {
                (Some(a), Some(b)) => match op {
                    CmpOp::Eq => (a - b).abs() < 1e-9,
                    CmpOp::Ne => (a - b).abs() >= 1e-9,
                    CmpOp::Gt => a > b,
                    CmpOp::Lt => a < b,
                    CmpOp::Ge => a >= b,
                    CmpOp::Le => a <= b,
                },
                // Fall back to string comparison for = and !=
                _ => {
                    let a = meta_val.as_str_repr();
                    let b = &pred.value;
                    match op {
                        CmpOp::Eq => &a == b,
                        CmpOp::Ne => &a != b,
                        _ => false,
                    }
                }
            }
        }
    }
}

fn node_matches_predicates(node: &IntentNode, predicates: &[Predicate]) -> bool {
    predicates.iter().all(|p| predicate_matches(&node.metadata, p))
}

// ═══════════════════════════════════════════════
//  Infrastructure
// ═══════════════════════════════════════════════

fn data_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ecphory")
}

fn fabric_path(project: &str) -> String {
    let dir = data_dir().join(project);
    std::fs::create_dir_all(&dir).ok();
    dir.join("fabric.json").to_string_lossy().to_string()
}

fn load_or_create_fabric(project: &str) -> (Fabric, String) {
    let path = fabric_path(project);
    let store = JsonFileStore::new(&path);
    let fabric = if store.exists() {
        store.load().unwrap_or_else(|e| {
            eprintln!("Warning: failed to load fabric: {}. Starting fresh.", e);
            Fabric::new()
        })
    } else {
        Fabric::new()
    };
    (fabric, path)
}

fn save_fabric(fabric: &Fabric, path: &str) {
    let store = JsonFileStore::new(path);
    store.save(fabric).unwrap_or_else(|e| {
        eprintln!("Error saving fabric: {}", e);
        process::exit(1);
    });
}

fn rebuild_embedder(fabric: &Fabric) -> BagOfWordsEmbedder {
    let docs: Vec<String> = fabric.nodes()
        .map(|(_, n)| n.want.description.clone())
        .collect();
    let mut embedder = BagOfWordsEmbedder::new();
    embedder.build_vocab_from_with_idf(&docs);
    embedder
}

fn parse_meta(args: &[String]) -> HashMap<String, MetadataValue> {
    let mut meta = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--meta" && i + 1 < args.len() {
            let kv = &args[i + 1];
            if let Some(eq_pos) = kv.find('=') {
                let key = kv[..eq_pos].to_string();
                let val = MetadataValue::parse(&kv[eq_pos + 1..]);
                meta.insert(key, val);
            } else {
                eprintln!("Warning: --meta value '{}' has no '=' separator, skipping", kv);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    meta
}

fn find_flag_value(args: &[String], flag: &str) -> Option<String> {
    for i in 0..args.len() {
        if args[i] == flag && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn print_node_line(id: &LineageId, node: &IntentNode, prefix: &str) {
    println!("{}[{}] {}", prefix, &id.as_uuid().to_string()[..8], node.want.description);
    if !node.metadata.is_empty() {
        let meta_prefix = format!("{}  ", prefix);
        print!("{}", meta_prefix);
        for (k, v) in &node.metadata {
            print!("{}={} ", k, v);
        }
        println!();
    }
}

// ═══════════════════════════════════════════════
//  Commands
// ═══════════════════════════════════════════════

fn cmd_fabric_add(args: &[String]) {
    let want = find_flag_value(args, "--want").unwrap_or_else(|| {
        eprintln!("Error: --want is required for 'fabric add'");
        process::exit(1);
    });
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let meta = parse_meta(args);
    let confidence: f64 = find_flag_value(args, "--confidence")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.7);

    let (mut fabric, path) = load_or_create_fabric(&project);

    let mut node = IntentNode::understood(&want, confidence);
    node.metadata = meta;

    let id = fabric.add_node(node);
    save_fabric(&fabric, &path);

    println!("Added node: {}", id);
    if let Some(node) = fabric.get_node(&id) {
        if !node.metadata.is_empty() {
            print!("  Metadata: ");
            for (k, v) in &node.metadata {
                print!("{}={} ", k, v);
            }
            println!();
        }
    }
}

fn cmd_fabric_list(args: &[String]) {
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let (fabric, _) = load_or_create_fabric(&project);

    if fabric.node_count() == 0 {
        println!("No nodes in project '{}'.", project);
        return;
    }

    println!("Nodes in project '{}' ({} total):", project, fabric.node_count());
    println!("{:-<70}", "");
    for (id, node) in fabric.nodes() {
        print_node_line(id, node, "  ");
    }
}

fn cmd_fabric_search(args: &[String]) {
    let query = find_flag_value(args, "--query");
    let where_clause = find_flag_value(args, "--where");
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let k: usize = find_flag_value(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    if query.is_none() && where_clause.is_none() {
        eprintln!("Error: --query and/or --where is required for 'fabric search'");
        process::exit(1);
    }

    let predicates = where_clause.as_deref()
        .map(parse_predicates)
        .unwrap_or_default();

    let (mut fabric, path) = load_or_create_fabric(&project);

    if let Some(ref q) = query {
        // Semantic search with optional predicate filter
        let embedder = rebuild_embedder(&fabric);
        fabric.set_embedder(Box::new(embedder));

        // Re-embed all nodes
        let ids: Vec<_> = fabric.nodes().map(|(id, _)| id.clone()).collect();
        for id in &ids {
            fabric.mutate_node(id, |_| {}).ok();
        }

        let results = fabric.resonate(q, k * 5); // Over-fetch to allow filtering

        let filtered: Vec<_> = results.iter()
            .filter(|r| {
                fabric.get_node(&r.lineage_id)
                    .map(|n| node_matches_predicates(n, &predicates))
                    .unwrap_or(false)
            })
            .take(k)
            .collect();

        if filtered.is_empty() {
            println!("No results found.");
        } else {
            println!("Search results ({} found):", filtered.len());
            println!("{:-<70}", "");
            for r in &filtered {
                if let Some(node) = fabric.get_node(&r.lineage_id) {
                    print!("  [{:.3}] ", r.score);
                    println!("{} — {}", &r.lineage_id.as_uuid().to_string()[..8], node.want.description);
                    if !node.metadata.is_empty() {
                        print!("         ");
                        for (k, v) in &node.metadata {
                            print!("{}={} ", k, v);
                        }
                        println!();
                    }
                }
            }
        }

        save_fabric(&fabric, &path);
    } else {
        // Pure predicate filter — no semantic query, return all matching sorted by recency
        let mut matches: Vec<(&LineageId, &IntentNode)> = fabric.nodes()
            .filter(|(_, n)| node_matches_predicates(n, &predicates))
            .collect();

        // Sort by lamport timestamp (most recent first) — use lineage_id order as fallback
        matches.sort_by(|(a_id, _), (b_id, _)| {
            let a_ts = fabric.node_lamport_ts(a_id).unwrap_or(0);
            let b_ts = fabric.node_lamport_ts(b_id).unwrap_or(0);
            b_ts.cmp(&a_ts)
        });

        let matches: Vec<_> = matches.into_iter().take(k).collect();

        if matches.is_empty() {
            println!("No matching nodes found.");
        } else {
            println!("Matching nodes ({} found):", matches.len());
            println!("{:-<70}", "");
            for (id, node) in &matches {
                print_node_line(id, node, "  ");
            }
        }
    }
}

fn cmd_fabric_aggregate(args: &[String]) {
    let field = find_flag_value(args, "--field").unwrap_or_else(|| {
        eprintln!("Error: --field is required for 'fabric aggregate'");
        process::exit(1);
    });
    let op = find_flag_value(args, "--op").unwrap_or_else(|| {
        eprintln!("Error: --op is required for 'fabric aggregate' (sum, avg, min, max, count)");
        process::exit(1);
    });
    let where_clause = find_flag_value(args, "--where");
    let group_by = find_flag_value(args, "--group-by");
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());

    let predicates = where_clause.as_deref()
        .map(parse_predicates)
        .unwrap_or_default();

    let (fabric, _) = load_or_create_fabric(&project);

    // Collect matching nodes
    let matching: Vec<&IntentNode> = fabric.nodes()
        .filter(|(_, n)| node_matches_predicates(n, &predicates))
        .map(|(_, n)| n)
        .collect();

    if let Some(ref group_key) = group_by {
        // Grouped aggregation
        let mut groups: HashMap<String, Vec<f64>> = HashMap::new();

        for node in &matching {
            let group_val = node.metadata.get(group_key.as_str())
                .map(|v| v.as_str_repr())
                .unwrap_or_else(|| "(none)".to_string());

            let field_val = if op == "count" {
                1.0
            } else {
                node.metadata.get(&field)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            };

            groups.entry(group_val).or_default().push(field_val);
        }

        let mut results: Vec<serde_json::Value> = Vec::new();
        for (group_val, values) in &groups {
            let result = compute_aggregate(&op, values);
            results.push(serde_json::json!({
                group_key.as_str(): group_val,
                "result": result
            }));
        }

        // Sort by group key
        results.sort_by(|a, b| {
            let ak = a.get(group_key.as_str()).and_then(|v| v.as_str()).unwrap_or("");
            let bk = b.get(group_key.as_str()).and_then(|v| v.as_str()).unwrap_or("");
            ak.cmp(bk)
        });

        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"groups": results})).unwrap());
    } else {
        // Flat aggregation
        let values: Vec<f64> = if op == "count" {
            vec![matching.len() as f64]
        } else {
            matching.iter()
                .filter_map(|n| n.metadata.get(&field).and_then(|v| v.as_f64()))
                .collect()
        };

        let result = if op == "count" {
            matching.len() as f64
        } else {
            compute_aggregate(&op, &values)
        };

        println!("{}", serde_json::json!({"result": result}));
    }
}

fn compute_aggregate(op: &str, values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    match op {
        "sum" => values.iter().sum(),
        "avg" => values.iter().sum::<f64>() / values.len() as f64,
        "min" => values.iter().cloned().fold(f64::INFINITY, f64::min),
        "max" => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        "count" => values.len() as f64,
        _ => {
            eprintln!("Unknown operation: {}. Use sum, avg, min, max, count.", op);
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Usage: intent <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  fabric add       --want \"...\" [--meta \"key=value\"]... [--project name]");
    eprintln!("  fabric list      [--project name]");
    eprintln!("  fabric search    [--query \"...\"] [--where \"key=value AND ...\"] [--project name]");
    eprintln!("  fabric aggregate --field F --op OP [--where \"...\"] [--group-by key] [--project name]");
    eprintln!();
    eprintln!("Predicate operators: =, !=, >, <, >=, <=");
    eprintln!("Aggregate operations: sum, avg, min, max, count");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        print_usage();
        process::exit(1);
    }

    let cmd1 = args[1].as_str();
    let cmd2 = args[2].as_str();

    match (cmd1, cmd2) {
        ("fabric", "add") => cmd_fabric_add(&args[3..]),
        ("fabric", "list") => cmd_fabric_list(&args[3..]),
        ("fabric", "search") => cmd_fabric_search(&args[3..]),
        ("fabric", "aggregate") => cmd_fabric_aggregate(&args[3..]),
        _ => {
            eprintln!("Unknown command: {} {}", cmd1, cmd2);
            print_usage();
            process::exit(1);
        }
    }
}

// ═══════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eq_predicate() {
        let preds = parse_predicates("domain=marketing");
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].key, "domain");
        assert_eq!(preds[0].op, CmpOp::Eq);
        assert_eq!(preds[0].value, "marketing");
    }

    #[test]
    fn parse_gt_predicate() {
        let preds = parse_predicates("cost>0.1");
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].op, CmpOp::Gt);
    }

    #[test]
    fn parse_ge_predicate() {
        let preds = parse_predicates("cost>=0.5");
        assert_eq!(preds[0].op, CmpOp::Ge);
        assert_eq!(preds[0].value, "0.5");
    }

    #[test]
    fn parse_ne_predicate() {
        let preds = parse_predicates("model!=haiku");
        assert_eq!(preds[0].op, CmpOp::Ne);
    }

    #[test]
    fn parse_and_predicates() {
        let preds = parse_predicates("domain=marketing AND cost>0.1");
        assert_eq!(preds.len(), 2);
        assert_eq!(preds[0].key, "domain");
        assert_eq!(preds[1].key, "cost");
    }

    #[test]
    fn predicate_matches_string_eq() {
        let mut meta = HashMap::new();
        meta.insert("domain".into(), MetadataValue::String("marketing".into()));
        let pred = Predicate { key: "domain".into(), op: CmpOp::Eq, value: "marketing".into() };
        assert!(predicate_matches(&meta, &pred));
    }

    #[test]
    fn predicate_matches_numeric_gt() {
        let mut meta = HashMap::new();
        meta.insert("cost".into(), MetadataValue::Float(0.42));
        let pred = Predicate { key: "cost".into(), op: CmpOp::Gt, value: "0.1".into() };
        assert!(predicate_matches(&meta, &pred));
    }

    #[test]
    fn predicate_matches_numeric_lt() {
        let mut meta = HashMap::new();
        meta.insert("cost".into(), MetadataValue::Float(0.05));
        let pred = Predicate { key: "cost".into(), op: CmpOp::Lt, value: "0.1".into() };
        assert!(predicate_matches(&meta, &pred));
    }

    #[test]
    fn predicate_missing_key_no_match() {
        let meta = HashMap::new();
        let pred = Predicate { key: "domain".into(), op: CmpOp::Eq, value: "marketing".into() };
        assert!(!predicate_matches(&meta, &pred));
    }

    #[test]
    fn predicate_int_comparison() {
        let mut meta = HashMap::new();
        meta.insert("tokens".into(), MetadataValue::Int(1500));
        let pred = Predicate { key: "tokens".into(), op: CmpOp::Ge, value: "1000".into() };
        assert!(predicate_matches(&meta, &pred));
    }

    #[test]
    fn compute_aggregate_sum() {
        assert!((compute_aggregate("sum", &[1.0, 2.0, 3.0]) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn compute_aggregate_avg() {
        assert!((compute_aggregate("avg", &[2.0, 4.0, 6.0]) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn compute_aggregate_min() {
        assert!((compute_aggregate("min", &[3.0, 1.0, 2.0]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn compute_aggregate_max() {
        assert!((compute_aggregate("max", &[3.0, 1.0, 2.0]) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn compute_aggregate_count() {
        assert!((compute_aggregate("count", &[1.0, 1.0, 1.0]) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn compute_aggregate_empty() {
        assert!((compute_aggregate("sum", &[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn node_matches_multiple_predicates() {
        let mut node = IntentNode::new("test");
        node.metadata.insert("domain".into(), MetadataValue::String("marketing".into()));
        node.metadata.insert("cost".into(), MetadataValue::Float(0.42));

        let preds = parse_predicates("domain=marketing AND cost>0.1");
        assert!(node_matches_predicates(&node, &preds));

        let preds2 = parse_predicates("domain=marketing AND cost>1.0");
        assert!(!node_matches_predicates(&node, &preds2));
    }

    #[test]
    fn predicate_bool_match() {
        let mut meta = HashMap::new();
        meta.insert("success".into(), MetadataValue::Bool(true));
        let pred = Predicate { key: "success".into(), op: CmpOp::Eq, value: "true".into() };
        assert!(predicate_matches(&meta, &pred));
    }
}
