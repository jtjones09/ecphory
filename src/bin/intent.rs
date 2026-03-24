// INTENT CLI — Bootstrap tool for the Ecphory Fabric
//
// Commands:
//   intent fabric add --want "..." [--meta "key=value"]... [--project name]
//   intent fabric list [--project name]
//   intent fabric search --query "..." [--project name]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process;

use ecphory::node::{IntentNode, MetadataValue};
use ecphory::persist::{FabricStore, JsonFileStore};
use ecphory::fabric::Fabric;
use ecphory::embedding::bow::BagOfWordsEmbedder;

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
        println!("  [{}] {}", &id.as_uuid().to_string()[..8], node.want.description);
        if !node.metadata.is_empty() {
            print!("    ");
            for (k, v) in &node.metadata {
                print!("{}={} ", k, v);
            }
            println!();
        }
    }
}

fn cmd_fabric_search(args: &[String]) {
    let query = find_flag_value(args, "--query").unwrap_or_else(|| {
        eprintln!("Error: --query is required for 'fabric search'");
        process::exit(1);
    });
    let project = find_flag_value(args, "--project").unwrap_or_else(|| "default".to_string());
    let k: usize = find_flag_value(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let (mut fabric, path) = load_or_create_fabric(&project);

    // Build embedder from current corpus
    let embedder = rebuild_embedder(&fabric);
    fabric.set_embedder(Box::new(embedder));

    // Re-embed all nodes
    let ids: Vec<_> = fabric.nodes().map(|(id, _)| id.clone()).collect();
    for id in &ids {
        fabric.mutate_node(id, |_| {}).ok();
    }

    let results = fabric.resonate(&query, k);

    if results.is_empty() {
        println!("No results for '{}' in project '{}'.", query, project);
    } else {
        println!("Search results for '{}' ({} found):", query, results.len());
        println!("{:-<70}", "");
        for r in &results {
            if let Some(node) = fabric.get_node(&r.lineage_id) {
                println!("  [{:.3}] {} — {}", r.score, &r.lineage_id.as_uuid().to_string()[..8], node.want.description);
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
}

fn print_usage() {
    eprintln!("Usage: intent <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  fabric add    --want \"...\" [--meta \"key=value\"]... [--project name] [--confidence 0.7]");
    eprintln!("  fabric list   [--project name]");
    eprintln!("  fabric search --query \"...\" [--project name] [--limit N]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --project    Project name (default: 'default')");
    eprintln!("  --meta       Metadata key=value pair (repeatable). Auto-detects type.");
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
        _ => {
            eprintln!("Unknown command: {} {}", cmd1, cmd2);
            print_usage();
            process::exit(1);
        }
    }
}
