//! Intent processing — operator lines become OperatorIntent nodes; the
//! fabric replies with FabricResponse nodes. Phase 1 recognises a small
//! command vocabulary; everything else is recorded as raw comms for
//! future agent processing.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::UPTIME_TICKS;
use crate::fabric::{EdgeKind, Fabric, NodeId, NodeKind};
use crate::generative_model::{operator_region, MODEL};
use crate::tesseract::TESSERACT;
use core::sync::atomic::Ordering;

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
    let (response_text, first_word, was_known) = handle_with_context(fabric, line);
    let r_lamport = fabric.tick();
    let response = fabric.create(NodeKind::FabricResponse {
        text: response_text.clone(),
        lamport: r_lamport,
    });
    fabric.link(intent, response, EdgeKind::Causes);

    // Feed the operator region so it learns command-vocabulary patterns.
    {
        let mut slot = MODEL.lock();
        if let Some(model) = slot.as_mut() {
            let classification = operator_region::classify_command(was_known, &first_word);
            let surprise = model.operator.observe_intent(classification);
            let cause_label = match classification {
                operator_region::OBS_HELP => "operator:help",
                operator_region::OBS_PERSIST => "operator:persist",
                operator_region::OBS_KNOWN => "operator:known",
                operator_region::OBS_UNKNOWN => "operator:unknown",
                _ => "operator:silence",
            };
            let cause_id = model
                .causal_graph
                .intern(cause_label, "operator");
            let effect_id = model
                .causal_graph
                .intern("response:rendered", "operator");
            model.causal_graph.record(cause_id, effect_id);
            model.account_observation(
                "operator",
                surprise,
                first_word.clone(),
            );
        }
    }

    Exchange {
        intent,
        response,
        response_text,
    }
}

fn handle_with_context(fabric: &Fabric, line: &str) -> (String, String, bool) {
    let trimmed = line.trim();
    let lower: String = trimmed.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut iter = lower.split_whitespace();
    let first = iter.next().unwrap_or("").to_string();
    let response = handle(fabric, line);
    // We treat any handled command (i.e. not the `unknown intent: ...`
    // branch) as "known".
    let was_known = !response.starts_with("unknown intent:");
    (response, first, was_known)
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
        "causal" | "graph" => command_causal(),
        "surprise" | "surp" => command_surprise(),
        "model" | "mind" => command_model(),
        "disks" | "blockio" => command_disks(),
        _ => format!("unknown intent: {} (try `help`)", trimmed),
    }
}

fn command_causal() -> String {
    let slot = MODEL.lock();
    let model = match slot.as_ref() {
        Some(m) => m,
        None => return "causal graph: model not yet nucleated".to_string(),
    };
    let nodes = model.causal_graph.node_count();
    let edges = model.causal_graph.edge_count();
    if edges == 0 {
        return format!("causal: {} nodes, {} edges (no relations yet)", nodes, edges);
    }
    let top = model.causal_graph.render_top_edges(8);
    let mut s = format!("causal: {} nodes, {} edges (top by strength):", nodes, edges);
    for line in top {
        s.push_str(&format!("\n  {}", line));
    }
    s
}

fn command_surprise() -> String {
    let slot = MODEL.lock();
    let model = match slot.as_ref() {
        Some(m) => m,
        None => return "surprise: model not yet nucleated".to_string(),
    };
    let avg = model.average_surprise();
    let current = model.history.current_fe().unwrap_or(0.0);
    let delta = model.history.delta_fe().unwrap_or(0.0);
    let mut s = format!(
        "surprise: F̄={:.3} F={:.3} ΔF={:.3} obs={}",
        avg, current, delta, model.total_observations,
    );
    if !model.history.surprises.is_empty() {
        s.push_str("\nrecent surprising:");
        let recent: Vec<&crate::generative_model::SurpriseEntry> =
            model.history.surprises.iter().rev().take(5).collect();
        for entry in recent {
            s.push_str(&format!(
                "\n  L{} {}: {:.2} ({})",
                entry.lamport, entry.region, entry.surprise, entry.note,
            ));
        }
    }
    s
}

fn command_model() -> String {
    let slot = MODEL.lock();
    let model = match slot.as_ref() {
        Some(m) => m,
        None => return "model: not yet nucleated".to_string(),
    };
    let mut s = model.render_overview();
    s.push_str("\n  ");
    s.push_str(&model.meta.render_summary());
    if let Some(d) = model.devices.storage() {
        s.push_str(&format!(
            "\n  device[{}]: {} (state {}, surp {:.2})",
            d.label,
            d.kind,
            d.map_state_label(),
            d.average_surprise(),
        ));
    }
    s.push_str(&format!(
        "\n  persistence: {}/{} ok ({}/{} restored), state {}",
        model.persistence.successful_persists,
        model.persistence.successful_persists + model.persistence.failed_persists,
        model.persistence.successful_restores,
        model.persistence.successful_restores + model.persistence.failed_restores,
        model.persistence.map_state_label(),
    ));
    s.push_str(&format!(
        "\n  operator: {} intents, {} unknown, {} help",
        model.operator.intents_seen,
        model.operator.unknown_commands,
        model.operator.help_requests,
    ));
    s
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
    // Bump the unsaved-information accumulator above any reasonable
    // threshold so the next inference cycle's `should_persist_now`
    // returns true. The model decides whether to persist; we just push
    // its parameter. Replaces the old AtomicBool flag with a model-
    // resident, learnable mechanism.
    let mut slot = MODEL.lock();
    if let Some(m) = slot.as_mut() {
        m.cumulative_surprise_since_last_persist += 100.0;
    }
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
    "commands: status health work agents fabric memory pci acpi features intents uptime persist clear causal surprise model disks help"
        .to_string()
}

fn command_disks() -> String {
    let inv = crate::storage_inventory::snapshot();
    if inv.is_empty() {
        return "disks: no BlockIO inventory recorded (substrate did not enumerate)".to_string();
    }
    let total = inv.len();
    let picked = inv.iter().filter(|d| d.picked_for_storage).count();
    let mut s = format!("disks: {} BlockIO devices (picked: {}):", total, picked);
    for d in inv {
        s.push_str(&format!("\n  {}", d.render_summary()));
    }
    s
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
