// INTENT CLI — Bootstrap tool for the Ecphory Fabric
//
// Commands:
//   intent fabric add       --want "..." [--meta "key=value"]... [--project name]
//   intent fabric list      [--project name]
//   intent fabric search    [--query "..."] [--where "key=value AND ..."] [--project name]
//   intent fabric aggregate --field F --op OP [--where "..."] [--group-by key] [--project name]
//   intent fabric stats     [--project name]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

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

// ═══════════════════════════════════════════════
//  Weight Decay — Retrieval IS Reinforcement
// ═══════════════════════════════════════════════

fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple ISO-ish timestamp from epoch seconds
    // Format: seconds since epoch as a string (parseable, sortable)
    // We use a proper format for human readability
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;
    // Approximate date calculation from epoch
    // Good enough for decay computation — not a calendar library
    let year = 1970 + (days / 365); // approximate
    let day_of_year = days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month.min(12), day.min(28), hours, minutes, seconds)
}

fn epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn parse_iso_to_epoch(iso: &str) -> Option<f64> {
    // Parse our ISO format: YYYY-MM-DDThh:mm:ssZ
    // Simple parser — extract components and approximate epoch
    let parts: Vec<&str> = iso.split('T').collect();
    if parts.len() != 2 { return None; }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<u64> = time_str.split(':').filter_map(|s| s.parse().ok()).collect();
    if date_parts.len() != 3 || time_parts.len() != 3 { return None; }
    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);
    let (hour, min, sec) = (time_parts[0], time_parts[1], time_parts[2]);
    // Approximate epoch calculation
    let days = (year - 1970) * 365 + (month - 1) * 30 + (day - 1);
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(secs as f64)
}

fn stamp_activation(node: &mut IntentNode) {
    node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
    let count = node.metadata.get("activation_count")
        .and_then(|v| if let MetadataValue::Int(i) = v { Some(*i) } else { None })
        .unwrap_or(0);
    node.metadata.insert("activation_count".into(), MetadataValue::Int(count + 1));
}

/// Compute composite weight for a node.
/// Formula: (comprehension * 0.3) + (temporal_recency * 0.3) + (activation_frequency * 0.2) + (resonance_score * 0.2)
fn compute_composite_weight(node: &IntentNode, resonance_score: f64, half_life_days: f64) -> f64 {
    // Comprehension from confidence surface
    let comprehension = node.confidence.comprehension.mean;

    // Temporal recency: exponential decay from last_activated
    let temporal_recency = if let Some(MetadataValue::String(ts)) = node.metadata.get("last_activated") {
        if let Some(activated_epoch) = parse_iso_to_epoch(ts) {
            let now = epoch_secs();
            let age_secs = (now - activated_epoch).max(0.0);
            let half_life_secs = half_life_days * 86400.0;
            let lambda = (2.0_f64.ln()) / half_life_secs;
            (-lambda * age_secs).exp()
        } else {
            0.5 // Can't parse timestamp, use neutral value
        }
    } else {
        1.0 // No last_activated = brand new node, full recency
    };

    // Activation frequency: log(activation_count + 1), normalized to [0, 1] range
    let activation_count = node.metadata.get("activation_count")
        .and_then(|v| if let MetadataValue::Int(i) = v { Some(*i) } else { None })
        .unwrap_or(0);
    let activation_freq = ((activation_count as f64 + 1.0).ln() / 10.0_f64.ln()).min(1.0);

    let raw = (comprehension * 0.3) + (temporal_recency * 0.3) + (activation_freq * 0.2) + (resonance_score * 0.2);

    // Systemic nodes (innate layer) never decay below 0.5
    let is_systemic = node.metadata.get("kind")
        .map(|v| v.as_str_repr() == "systemic")
        .unwrap_or(false);
    if is_systemic {
        raw.max(0.5)
    } else {
        raw
    }
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
//  Systemic Intent Nodes
// ═══════════════════════════════════════════════

struct SystemicDef {
    want: &'static str,
    domain: &'static str,
    category: &'static str,
}

const SYSTEMIC_NODES: &[SystemicDef] = &[
    SystemicDef {
        want: "Maintain fabric coherence — no duplicate knowledge, no contradictions",
        domain: "system",
        category: "coherence",
    },
    SystemicDef {
        want: "Track all operations — every API call, every tool use, every decision",
        domain: "system_telemetry",
        category: "self_monitoring",
    },
    SystemicDef {
        want: "Strengthen high-value knowledge — frequently accessed nodes should be easy to find",
        domain: "system",
        category: "optimization",
    },
    SystemicDef {
        want: "Identify knowledge gaps — notice when queries return low-confidence results",
        domain: "system",
        category: "growth",
    },
    SystemicDef {
        want: "Preserve context across sessions — key decisions and reasoning should persist",
        domain: "system",
        category: "continuity",
    },
];

fn has_systemic_nodes(fabric: &Fabric) -> bool {
    fabric.nodes().any(|(_, n)| {
        n.metadata.get("kind")
            .map(|v| v.as_str_repr() == "systemic")
            .unwrap_or(false)
    })
}

fn seed_systemic_nodes(fabric: &mut Fabric) -> usize {
    let mut count = 0;
    for def in SYSTEMIC_NODES {
        let mut node = IntentNode::understood(def.want, 0.9);
        node.metadata.insert("kind".into(), MetadataValue::String("systemic".into()));
        node.metadata.insert("domain".into(), MetadataValue::String(def.domain.into()));
        node.metadata.insert("category".into(), MetadataValue::String(def.category.into()));
        node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
        node.metadata.insert("activation_count".into(), MetadataValue::Int(0));
        fabric.add_node(node);
        count += 1;
    }
    count
}

fn is_systemic(node: &IntentNode) -> bool {
    node.metadata.get("kind")
        .map(|v| v.as_str_repr() == "systemic")
        .unwrap_or(false)
}

// ═══════════════════════════════════════════════
//  Commands
// ═══════════════════════════════════════════════

fn cmd_fabric_init(args: &[String]) {
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let (mut fabric, path) = load_or_create_fabric(&project);

    if has_systemic_nodes(&fabric) {
        println!("Project '{}' already has systemic nodes. Skipping.", project);
        return;
    }

    let count = seed_systemic_nodes(&mut fabric);
    save_fabric(&fabric, &path);
    println!("Seeded {} systemic intent nodes for project '{}'.", count, project);
}

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

    // Auto-seed systemic nodes on first use of a project
    if !has_systemic_nodes(&fabric) {
        let seeded = seed_systemic_nodes(&mut fabric);
        if seeded > 0 {
            eprintln!("Initialized {} systemic nodes for project '{}'.", seeded, project);
        }
    }

    let mut node = IntentNode::understood(&want, confidence);
    node.metadata = meta;
    // Stamp initial activation
    node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
    node.metadata.insert("activation_count".into(), MetadataValue::Int(0));

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
    let systemic_only = args.iter().any(|a| a == "--systemic");
    let (fabric, _) = load_or_create_fabric(&project);

    if fabric.node_count() == 0 {
        println!("No nodes in project '{}'.", project);
        return;
    }

    let nodes: Vec<_> = fabric.nodes()
        .filter(|(_, n)| !systemic_only || is_systemic(n))
        .collect();

    if nodes.is_empty() {
        println!("No {} nodes in project '{}'.", if systemic_only { "systemic" } else { "" }, project);
        return;
    }

    let label = if systemic_only { "Systemic nodes" } else { "Nodes" };
    println!("{} in project '{}' ({} total):", label, project, nodes.len());
    println!("{:-<70}", "");
    for (id, node) in &nodes {
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
    let half_life: f64 = find_flag_value(args, "--decay-halflife")
        .and_then(|s| s.parse().ok())
        .unwrap_or(7.0); // Default: 7 days

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

        // Filter by predicates and compute composite weight
        let mut scored: Vec<(LineageId, f64, f64)> = Vec::new(); // (id, composite_weight, resonance)
        for r in &results {
            if let Some(node) = fabric.get_node(&r.lineage_id) {
                if node_matches_predicates(node, &predicates) {
                    let cw = compute_composite_weight(node, r.score, half_life);
                    scored.push((r.lineage_id.clone(), cw, r.score));
                }
            }
        }

        // Sort by composite_weight descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let scored: Vec<_> = scored.into_iter().take(k).collect();

        // Stamp activation on returned nodes (retrieval IS reinforcement)
        for (id, _, _) in &scored {
            fabric.mutate_node(id, |n| stamp_activation(n)).ok();
        }

        if scored.is_empty() {
            println!("No results found.");
        } else {
            println!("Search results ({} found):", scored.len());
            println!("{:-<70}", "");
            for (id, cw, res) in &scored {
                if let Some(node) = fabric.get_node(id) {
                    println!("  [w:{:.3} r:{:.3}] {} — {}",
                        cw, res, &id.as_uuid().to_string()[..8], node.want.description);
                    if !node.metadata.is_empty() {
                        print!("         ");
                        for (k, v) in &node.metadata {
                            if k != "last_activated" && k != "activation_count" {
                                print!("{}={} ", k, v);
                            }
                        }
                        println!();
                    }
                }
            }
        }

        save_fabric(&fabric, &path);
    } else {
        // Pure predicate filter — sort by composite_weight
        let mut scored: Vec<(LineageId, f64)> = Vec::new();
        for (id, node) in fabric.nodes() {
            if node_matches_predicates(node, &predicates) {
                let cw = compute_composite_weight(node, 0.0, half_life);
                scored.push((id.clone(), cw));
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let scored: Vec<_> = scored.into_iter().take(k).collect();

        // Stamp activation
        for (id, _) in &scored {
            fabric.mutate_node(id, |n| stamp_activation(n)).ok();
        }

        if scored.is_empty() {
            println!("No matching nodes found.");
        } else {
            println!("Matching nodes ({} found):", scored.len());
            println!("{:-<70}", "");
            for (id, cw) in &scored {
                if let Some(node) = fabric.get_node(id) {
                    print!("  [w:{:.3}] ", cw);
                    println!("{} — {}", &id.as_uuid().to_string()[..8], node.want.description);
                    if !node.metadata.is_empty() {
                        print!("         ");
                        for (k, v) in &node.metadata {
                            if k != "last_activated" && k != "activation_count" {
                                print!("{}={} ", k, v);
                            }
                        }
                        println!();
                    }
                }
            }
        }

        save_fabric(&fabric, &path);
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

fn cmd_fabric_stats(args: &[String]) {
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let half_life: f64 = find_flag_value(args, "--decay-halflife")
        .and_then(|s| s.parse().ok())
        .unwrap_or(7.0);

    let (fabric, _) = load_or_create_fabric(&project);

    let count = fabric.node_count();
    if count == 0 {
        println!("No nodes in project '{}'.", project);
        return;
    }

    // Compute weights for all nodes
    let mut weights: Vec<(String, String, f64, i64)> = Vec::new(); // (id, want, weight, activation_count)
    let mut oldest_name = String::new();
    let mut oldest_ts = f64::MAX;
    let mut most_activated_name = String::new();
    let mut most_activated_count: i64 = -1;
    let mut least_activated_name = String::new();
    let mut least_activated_count: i64 = i64::MAX;

    for (id, node) in fabric.nodes() {
        let cw = compute_composite_weight(node, 0.0, half_life);
        let act_count = node.metadata.get("activation_count")
            .and_then(|v| if let MetadataValue::Int(i) = v { Some(*i) } else { None })
            .unwrap_or(0);

        let short_id = id.as_uuid().to_string()[..8].to_string();
        let desc = if node.want.description.len() > 40 {
            format!("{}...", &node.want.description[..37])
        } else {
            node.want.description.clone()
        };

        weights.push((short_id, desc.clone(), cw, act_count));

        // Track oldest
        if let Some(MetadataValue::String(ts)) = node.metadata.get("last_activated") {
            if let Some(epoch) = parse_iso_to_epoch(ts) {
                if epoch < oldest_ts {
                    oldest_ts = epoch;
                    oldest_name = desc.clone();
                }
            }
        }

        // Track most/least activated
        if act_count > most_activated_count {
            most_activated_count = act_count;
            most_activated_name = desc.clone();
        }
        if act_count < least_activated_count {
            least_activated_count = act_count;
            least_activated_name = desc.clone();
        }
    }

    // Sort by weight descending
    weights.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    println!("Fabric Stats for '{}' ({} nodes):", project, count);
    println!("{:=<70}", "");

    // Weight distribution histogram
    let mut buckets = [0u32; 10]; // [0.0-0.1, 0.1-0.2, ..., 0.9-1.0]
    for (_, _, w, _) in &weights {
        let bucket = ((*w * 10.0).floor() as usize).min(9);
        buckets[bucket] += 1;
    }

    println!("\n  Weight Distribution:");
    for i in (0..10).rev() {
        let lower = i as f64 / 10.0;
        let upper = (i + 1) as f64 / 10.0;
        let bar: String = "█".repeat(buckets[i] as usize);
        if buckets[i] > 0 {
            println!("  {:.1}-{:.1} | {} ({})", lower, upper, bar, buckets[i]);
        }
    }

    println!("\n  Top 5 by weight:");
    for (id, desc, w, ac) in weights.iter().take(5) {
        println!("    [{:.3}] {} — {} (activated {} times)", w, id, desc, ac);
    }

    println!("\n  Bottom 5 by weight:");
    for (id, desc, w, ac) in weights.iter().rev().take(5) {
        println!("    [{:.3}] {} — {} (activated {} times)", w, id, desc, ac);
    }

    println!("\n  Highlights:");
    if !oldest_name.is_empty() {
        println!("    Oldest node: {}", oldest_name);
    }
    if most_activated_count >= 0 {
        println!("    Most activated: {} ({} times)", most_activated_name, most_activated_count);
    }
    if least_activated_count < i64::MAX {
        println!("    Least activated: {} ({} times)", least_activated_name, least_activated_count);
    }
}

fn print_usage() {
    eprintln!("Usage: intent <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  fabric init      [--project name]  — seed systemic intent nodes");
    eprintln!("  fabric add       --want \"...\" [--meta \"key=value\"]... [--project name]");
    eprintln!("  fabric list      [--project name] [--systemic]");
    eprintln!("  fabric search    [--query \"...\"] [--where \"...\"] [--decay-halflife N] [--project name]");
    eprintln!("  fabric aggregate --field F --op OP [--where \"...\"] [--group-by key] [--project name]");
    eprintln!("  fabric stats     [--project name] [--decay-halflife N]");
    eprintln!();
    eprintln!("Predicate operators: =, !=, >, <, >=, <=");
    eprintln!("Aggregate operations: sum, avg, min, max, count");
    eprintln!("Decay half-life: days (default 7). Controls temporal recency weighting.");
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
        ("fabric", "init") => cmd_fabric_init(&args[3..]),
        ("fabric", "add") => cmd_fabric_add(&args[3..]),
        ("fabric", "list") => cmd_fabric_list(&args[3..]),
        ("fabric", "search") => cmd_fabric_search(&args[3..]),
        ("fabric", "aggregate") => cmd_fabric_aggregate(&args[3..]),
        ("fabric", "stats") => cmd_fabric_stats(&args[3..]),
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

    // ── Weight Decay Tests ──

    #[test]
    fn new_node_has_composite_weight() {
        let mut node = IntentNode::understood("test", 0.8);
        node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
        node.metadata.insert("activation_count".into(), MetadataValue::Int(0));
        let cw = compute_composite_weight(&node, 0.5, 7.0);
        assert!(cw > 0.0, "New node should have positive composite weight");
    }

    #[test]
    fn retrieved_node_has_higher_activation() {
        let mut node = IntentNode::understood("test", 0.8);
        node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
        node.metadata.insert("activation_count".into(), MetadataValue::Int(0));

        let cw1 = compute_composite_weight(&node, 0.5, 7.0);

        // Simulate retrieval
        stamp_activation(&mut node);
        stamp_activation(&mut node);
        stamp_activation(&mut node);

        let cw2 = compute_composite_weight(&node, 0.5, 7.0);
        assert!(cw2 > cw1, "Node retrieved multiple times should have higher weight. cw1={}, cw2={}", cw1, cw2);
    }

    #[test]
    fn old_node_has_lower_temporal_recency() {
        // Node activated 14 days ago
        let mut old_node = IntentNode::understood("old test", 0.8);
        // Simulate 14 days ago: subtract 14*86400 seconds
        let old_epoch = epoch_secs() - 14.0 * 86400.0;
        let days = (old_epoch / 86400.0).floor() as u64;
        let remaining = (old_epoch % 86400.0) as u64;
        let year = 1970 + days / 365;
        let day_of_year = days % 365;
        let month = day_of_year / 30 + 1;
        let day = day_of_year % 30 + 1;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        let seconds = remaining % 60;
        let old_ts = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month.min(12), day.min(28), hours, minutes, seconds);
        old_node.metadata.insert("last_activated".into(), MetadataValue::String(old_ts));
        old_node.metadata.insert("activation_count".into(), MetadataValue::Int(0));

        // Fresh node
        let mut fresh_node = IntentNode::understood("fresh test", 0.8);
        fresh_node.metadata.insert("last_activated".into(), MetadataValue::String(now_iso()));
        fresh_node.metadata.insert("activation_count".into(), MetadataValue::Int(0));

        let cw_old = compute_composite_weight(&old_node, 0.5, 7.0);
        let cw_fresh = compute_composite_weight(&fresh_node, 0.5, 7.0);
        assert!(cw_fresh > cw_old, "Fresh node should have higher weight. fresh={}, old={}", cw_fresh, cw_old);
    }

    #[test]
    fn high_activation_compensates_low_recency() {
        // Old node but highly activated
        let mut old_active = IntentNode::understood("old but popular", 0.8);
        let old_epoch = epoch_secs() - 14.0 * 86400.0;
        let days = (old_epoch / 86400.0).floor() as u64;
        let remaining = (old_epoch % 86400.0) as u64;
        let year = 1970 + days / 365;
        let day_of_year = days % 365;
        let month = day_of_year / 30 + 1;
        let day = day_of_year % 30 + 1;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        let seconds = remaining % 60;
        let old_ts = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month.min(12), day.min(28), hours, minutes, seconds);
        old_active.metadata.insert("last_activated".into(), MetadataValue::String(old_ts));
        old_active.metadata.insert("activation_count".into(), MetadataValue::Int(50));

        let cw = compute_composite_weight(&old_active, 0.5, 7.0);
        assert!(cw > 0.3, "Old but highly activated node should still have meaningful weight: {}", cw);
    }

    #[test]
    fn stamp_activation_increments_count() {
        let mut node = IntentNode::new("test");
        node.metadata.insert("activation_count".into(), MetadataValue::Int(3));
        stamp_activation(&mut node);
        assert_eq!(node.metadata.get("activation_count"), Some(&MetadataValue::Int(4)));
    }

    #[test]
    fn stamp_activation_updates_timestamp() {
        let mut node = IntentNode::new("test");
        stamp_activation(&mut node);
        assert!(node.metadata.contains_key("last_activated"));
    }

    #[test]
    fn parse_iso_roundtrip() {
        let ts = now_iso();
        let epoch = parse_iso_to_epoch(&ts);
        assert!(epoch.is_some(), "Should parse our own timestamp format");
        let now = epoch_secs();
        // Should be within 60 seconds of now
        assert!((epoch.unwrap() - now).abs() < 60.0, "Parsed time should be close to current time");
    }

    // ── Systemic Node Tests ──

    #[test]
    fn systemic_nodes_seeded_on_empty_fabric() {
        let mut fabric = Fabric::new();
        assert!(!has_systemic_nodes(&fabric));
        let count = seed_systemic_nodes(&mut fabric);
        assert_eq!(count, 5);
        assert!(has_systemic_nodes(&fabric));
    }

    #[test]
    fn systemic_nodes_not_duplicated() {
        let mut fabric = Fabric::new();
        seed_systemic_nodes(&mut fabric);
        assert_eq!(fabric.node_count(), 5);
        // Check that has_systemic_nodes returns true, preventing re-seeding
        assert!(has_systemic_nodes(&fabric));
    }

    #[test]
    fn systemic_nodes_have_correct_metadata() {
        let mut fabric = Fabric::new();
        seed_systemic_nodes(&mut fabric);
        for (_, node) in fabric.nodes() {
            assert_eq!(node.metadata.get("kind"), Some(&MetadataValue::String("systemic".into())));
            assert!(node.metadata.contains_key("domain"));
            assert!(node.metadata.contains_key("category"));
        }
    }

    #[test]
    fn systemic_nodes_have_high_confidence() {
        let mut fabric = Fabric::new();
        seed_systemic_nodes(&mut fabric);
        for (_, node) in fabric.nodes() {
            assert!((node.confidence.comprehension.mean - 0.9).abs() < 0.01);
        }
    }

    #[test]
    fn systemic_nodes_minimum_weight_floor() {
        let mut node = IntentNode::understood("test systemic", 0.1);
        node.metadata.insert("kind".into(), MetadataValue::String("systemic".into()));
        // Even with low confidence and old timestamp, weight should be >= 0.5
        node.metadata.insert("last_activated".into(), MetadataValue::String("2020-01-01T00:00:00Z".into()));
        node.metadata.insert("activation_count".into(), MetadataValue::Int(0));
        let cw = compute_composite_weight(&node, 0.0, 7.0);
        assert!(cw >= 0.5, "Systemic node weight should never go below 0.5, got {}", cw);
    }

    #[test]
    fn non_systemic_nodes_can_decay_below_half() {
        let mut node = IntentNode::understood("test regular", 0.1);
        node.metadata.insert("last_activated".into(), MetadataValue::String("2020-01-01T00:00:00Z".into()));
        node.metadata.insert("activation_count".into(), MetadataValue::Int(0));
        let cw = compute_composite_weight(&node, 0.0, 7.0);
        assert!(cw < 0.5, "Regular node should be able to decay below 0.5, got {}", cw);
    }

    #[test]
    fn is_systemic_correct() {
        let mut node = IntentNode::new("test");
        assert!(!is_systemic(&node));
        node.metadata.insert("kind".into(), MetadataValue::String("systemic".into()));
        assert!(is_systemic(&node));
    }
}
