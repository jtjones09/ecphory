//! Intent processing — operator lines become OperatorIntent nodes; the
//! fabric replies with FabricResponse nodes. Phase 1 recognises a small
//! command vocabulary; everything else is recorded as raw comms for
//! future agent processing.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::UPTIME_TICKS;
use crate::fabric::{EdgeKind, Fabric, NodeId, NodeKind};
use crate::tesseract::TESSERACT;
use core::sync::atomic::Ordering;

use core::sync::atomic::AtomicBool;
pub static PERSIST_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct Exchange {
    pub intent: NodeId,
    pub response: NodeId,
    pub response_text: String,
}

pub fn submit(fabric: &mut Fabric, line: &str) -> Exchange {
    let lamport = fabric.tick();
    let intent = fabric.create(NodeKind::OperatorIntent {
        text: line.to_string(),
        lamport,
    });
    let response_text = handle(fabric, line);
    let r_lamport = fabric.tick();
    let response = fabric.create(NodeKind::FabricResponse {
        text: response_text.clone(),
        lamport: r_lamport,
    });
    fabric.link(intent, response, EdgeKind::Causes);
    Exchange {
        intent,
        response,
        response_text,
    }
}

fn handle(fabric: &Fabric, line: &str) -> String {
    let trimmed = line.trim();
    let lower: String = trimmed.chars().map(|c| c.to_ascii_lowercase()).collect();
    let parts: Vec<&str> = lower.split_whitespace().collect();
    match parts.first().copied().unwrap_or("") {
        "" => "(empty intent — fabric silent)".to_string(),
        "status" | "stat" => command_status(fabric),
        "health" | "immune" => command_health(fabric),
        "work" => command_work(fabric),
        "help" | "?" => command_help(),
        "memory" | "mem" => command_memory(fabric),
        "pci" => command_pci(fabric),
        "acpi" => command_acpi(fabric),
        "feat" | "features" => command_features(fabric),
        "uptime" => command_uptime(fabric),
        "agents" | "agent" => command_agents(fabric),
        "fabric" | "f" => command_fabric(fabric),
        "persist" | "save" => command_persist(),
        "clear" | "cls" => command_clear(),
        "intents" | "log" => command_intents(fabric),
        _ => format!("unknown intent: {} (try `help`)", trimmed),
    }
}

fn command_agents(f: &Fabric) -> String {
    // Agents are exposed as fabric SystemEvent + LearnedDriver nodes; we
    // count and summarise.
    let learned: Vec<&str> = f
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::LearnedDriver { kind, .. } => Some(kind.as_str()),
            _ => None,
        })
        .collect();
    let agent_events = f
        .iter_kind(10)
        .filter(|n| match &n.kind {
            NodeKind::SystemEvent { text, .. } => text.starts_with("storage:"),
            _ => false,
        })
        .count();
    if learned.is_empty() && agent_events == 0 {
        return "no active inference agents in the fabric yet".to_string();
    }
    format!(
        "agents: {} learned-drivers ({}), {} step events",
        learned.len(),
        learned.join(","),
        agent_events
    )
}

fn command_fabric(f: &Fabric) -> String {
    let total = f.nodes.len();
    let mut buckets: alloc::collections::BTreeMap<u8, usize> =
        alloc::collections::BTreeMap::new();
    for n in &f.nodes {
        *buckets.entry(n.kind.tag()).or_insert(0) += 1;
    }
    let mut entries: Vec<(u8, usize)> = buckets.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    let mut breakdown = String::new();
    for (tag, count) in entries.iter().take(8) {
        breakdown.push_str(&format!(" {}:{}", node_kind_name(*tag), count));
    }
    let total_weight: f32 = f.nodes.iter().map(|n| n.weight).sum();
    let avg_w = if total > 0 {
        total_weight / total as f32
    } else {
        0.0
    };
    format!(
        "fabric: {} nodes, {} edges, lamport={}, avg_w={:.3}, top:{}",
        total,
        f.edges.len(),
        f.lamport,
        avg_w,
        breakdown
    )
}

fn command_persist() -> String {
    PERSIST_REQUESTED.store(true, Ordering::Release);
    "persist requested — snapshot will be written this cycle".to_string()
}

fn command_clear() -> String {
    let mut t = TESSERACT.lock();
    t.log.clear();
    t.dirty = true;
    "interaction log cleared".to_string()
}

fn command_intents(f: &Fabric) -> String {
    let mut intents: Vec<&str> = f
        .iter_kind(8)
        .filter_map(|n| match &n.kind {
            NodeKind::OperatorIntent { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if intents.is_empty() {
        return "no operator intents recorded".to_string();
    }
    intents.reverse(); // newest first
    let take = intents.iter().take(8);
    let mut s = format!("intents ({} total, newest first):", intents.len());
    for t in take {
        s.push_str(&format!("\n  {}", t));
    }
    s
}

fn node_kind_name(tag: u8) -> &'static str {
    match tag {
        0 => "genesis",
        1 => "cpu",
        2 => "feat",
        3 => "mem",
        4 => "pci",
        5 => "acpi",
        6 => "fb",
        7 => "store",
        8 => "intent",
        9 => "resp",
        10 => "event",
        11 => "agent",
        _ => "?",
    }
}

fn command_status(f: &Fabric) -> String {
    let n = f.nodes.len();
    let e = f.edges.len();
    let cpu = f
        .iter_kind(1)
        .next()
        .and_then(|n| match &n.kind {
            NodeKind::HwCpu { vendor, brand } => Some(format!("{} / {}", vendor, brand)),
            _ => None,
        })
        .unwrap_or_else(|| "<unknown>".to_string());
    let pci = f.count_by_tag(4);
    let acpi = f.count_by_tag(5);
    let mem = f.count_by_tag(3);
    let intents = f.count_by_tag(8);
    format!(
        "fabric: {} nodes, {} edges, lamport {} | cpu={} | mem={} pci={} acpi={} intents={}",
        n, e, f.lamport, cpu, mem, pci, acpi, intents
    )
}

fn command_health(f: &Fabric) -> String {
    let mut total_weight = 0.0f32;
    let mut max_weight = 0.0f32;
    let mut min_weight = f32::MAX;
    for n in &f.nodes {
        total_weight += n.weight;
        if n.weight > max_weight {
            max_weight = n.weight;
        }
        if n.weight < min_weight {
            min_weight = n.weight;
        }
    }
    let count = f.nodes.len() as f32;
    let avg = if count > 0.0 { total_weight / count } else { 0.0 };
    let health = if avg > 0.7 {
        "green"
    } else if avg > 0.3 {
        "yellow"
    } else {
        "red"
    };
    format!(
        "immune: {} (avg weight {:.3}, range {:.3}..{:.3})",
        health, avg, min_weight, max_weight
    )
}

fn command_work(f: &Fabric) -> String {
    let intents = f.iter_kind(8).count();
    let responses = f.iter_kind(9).count();
    let pending = intents.saturating_sub(responses);
    format!(
        "work: {} intents, {} responses, {} pending",
        intents, responses, pending
    )
}

fn command_help() -> String {
    "commands: status health work agents fabric memory pci acpi features intents uptime persist clear help"
        .to_string()
}

fn command_memory(f: &Fabric) -> String {
    let mut total = 0u64;
    let mut usable = 0u64;
    let mut count = 0;
    for n in f.iter_kind(3) {
        if let NodeKind::HwMemoryRegion { start, end, kind } = &n.kind {
            total += end - start;
            if kind == "usable" {
                usable += end - start;
            }
            count += 1;
        }
    }
    format!(
        "memory: {} regions, total {} MiB, usable {} MiB",
        count,
        total / (1024 * 1024),
        usable / (1024 * 1024)
    )
}

fn command_pci(f: &Fabric) -> String {
    let mut out = format!("pci: {} devices", f.count_by_tag(4));
    for n in f.iter_kind(4).take(8) {
        if let NodeKind::HwPciDevice {
            bus,
            device,
            function,
            vendor_id,
            device_id,
            class,
            subclass,
            ..
        } = &n.kind
        {
            out.push_str(&format!(
                "\n  {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}:{:02x}",
                bus, device, function, vendor_id, device_id, class, subclass
            ));
        }
    }
    out
}

fn command_acpi(f: &Fabric) -> String {
    let mut sigs: Vec<&str> = Vec::new();
    for n in f.iter_kind(5) {
        if let NodeKind::HwAcpiTable { signature, .. } = &n.kind {
            if let Ok(s) = core::str::from_utf8(signature) {
                sigs.push(s);
            }
        }
    }
    format!("acpi: {} tables — {}", sigs.len(), sigs.join(" "))
}

fn command_features(f: &Fabric) -> String {
    let mut names: Vec<&str> = Vec::new();
    for n in f.iter_kind(2) {
        if let NodeKind::HwCpuFeature(name) = &n.kind {
            names.push(name.as_str());
        }
    }
    format!("cpu features: {}", names.join(" "))
}

fn command_uptime(f: &Fabric) -> String {
    let ticks = UPTIME_TICKS.load(Ordering::Relaxed);
    format!("uptime: lamport={} ticks={}", f.lamport, ticks)
}
