//! The active-inference main loop. Substrate-independent: takes a
//! `&mut dyn Shim` and drives observe → act → render → persist forever.
//!
//! Per the nucleation refactor, the loop now drives one
//! [`GenerativeModel`](crate::generative_model::GenerativeModel) instead
//! of a free-standing storage agent. The storage controller is one
//! region inside the model (`devices.storage_mut()`); persistence,
//! operator interaction, and meta-assessment are sibling regions on
//! the same struct. Step 3 of the plan keeps the existing
//! `LearnedDriver` fabric-node persistence pathway alive (forward-
//! compatibility shim) while Step 7 swaps the snapshot pathway over to
//! the model's full serialise/deserialise.

use alloc::format;
use alloc::string::{String, ToString};

use crate::fabric::FABRIC;
use crate::framebuffer::FrameBufferWriter;
use crate::generative_model::{DeviceModel, GenerativeModel, MODEL};
use crate::intent;
use crate::keyboard::LineEditor;
use crate::ops::{Op, OpResult, Shim};
use crate::snapshot;
use crate::tesseract::{TESSERACT, render};
use crate::NodeKind;

/// How often the storage agent runs one full step. Keeping it
/// throttled — every Nth iteration — leaves headroom for input
/// latency.
const AGENT_STEP_EVERY_TICK: u64 = 16;
const PERSIST_EVERY_LAMPORT: u64 = 5;
/// How often the meta-region reassesses the rest of the model. The
/// position paper suggests every 100 primary cycles.
const META_ASSESS_EVERY_LAMPORT: u64 = 100;

pub struct LoopConfig {
    pub agent_label: String,
    pub agent_enabled: bool,
    /// If true, the loop persists the fabric (and the agent's matrices)
    /// to the substrate's storage via the shim's WriteBlock op.
    pub persist_enabled: bool,
}

pub fn run<S: Shim + ?Sized>(
    shim: &mut S,
    fb: &spin::Mutex<Option<FrameBufferWriter>>,
    cfg: LoopConfig,
) -> ! {
    let mut editor = LineEditor::new();
    let mut last_input_render = String::new();
    let mut last_persist_attempt: u64 = u64::MAX;
    let mut last_agent_tick: u64 = 0;
    let mut last_meta_lamport: u64 = 0;
    let mut intent_pending_persist = false;

    // ---- Nucleate the GenerativeModel and its storage device. ----
    {
        let mut slot = MODEL.lock();
        if slot.is_none() {
            *slot = Some(GenerativeModel::nucleation());
        }
        if let Some(model) = slot.as_mut() {
            model.note_boot();
            if cfg.agent_enabled && model.devices.storage().is_none() {
                model.devices.add(DeviceModel::storage(cfg.agent_label.clone()));
            }
        }
    }

    // Try to restore from the fabric snapshot. We prefer a full-model
    // LearnedDriver (kind="model") if one exists — that restores every
    // region's beliefs, the causal graph, the meta-region's history, and
    // the global counters. If no full-model snapshot exists (older images
    // with only per-device LearnedDriver), fall back to restoring just
    // the storage device's matrices.
    let (model_blob, storage_blob) = {
        let f = FABRIC.lock();
        let mut model_latest: Option<(u64, alloc::vec::Vec<u8>)> = None;
        let mut storage_latest: Option<(u64, alloc::vec::Vec<u8>)> = None;
        for n in f.iter_kind(11) {
            if let NodeKind::LearnedDriver { kind, params, .. } = &n.kind {
                match kind.as_str() {
                    "model" => {
                        let take = match model_latest {
                            None => true,
                            Some((c, _)) => n.created_at > c,
                        };
                        if take {
                            model_latest = Some((n.created_at, params.clone()));
                        }
                    }
                    "storage" => {
                        let take = match storage_latest {
                            None => true,
                            Some((c, _)) => n.created_at > c,
                        };
                        if take {
                            storage_latest = Some((n.created_at, params.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        (model_latest, storage_latest)
    };

    let continued: bool = {
        let mut slot = MODEL.lock();
        if let Some((_, blob)) = model_blob.as_ref() {
            if let Some(mut restored) = GenerativeModel::deserialize_from_bytes(blob) {
                if cfg.agent_enabled && restored.devices.storage().is_none() {
                    restored
                        .devices
                        .add(DeviceModel::storage(cfg.agent_label.clone()));
                }
                restored.boot_count = restored.boot_count.saturating_add(1);
                *slot = Some(restored);
                true
            } else {
                false
            }
        } else if let (Some(model), Some((_, params))) =
            (slot.as_mut(), storage_blob.as_ref())
        {
            if let Some(d) = model.devices.storage_mut() {
                d.restore_from_bytes(params)
            } else {
                false
            }
        } else {
            false
        }
    };

    {
        let mut f = FABRIC.lock();
        let lamport = f.lamport;
        let label_text = {
            let slot = MODEL.lock();
            slot.as_ref()
                .and_then(|m| m.devices.storage())
                .map(|d| {
                    if continued {
                        format!(
                            "storage-agent: continued for {} (obs={} avg_surp={:.3})",
                            d.label,
                            d.observations_seen(),
                            d.average_surprise()
                        )
                    } else {
                        format!("storage-agent: spawned fresh for {}", d.label)
                    }
                })
                .unwrap_or_else(|| "storage-agent: not active".to_string())
        };
        f.create(NodeKind::SystemEvent {
            text: label_text,
            lamport,
        });
    }

    // Initial probe so median-latency is non-zero.
    if cfg.agent_enabled {
        let mut slot = MODEL.lock();
        if let Some(model) = slot.as_mut() {
            if let Some(d) = model.devices.storage_mut() {
                let _ = d.agent.step(shim);
            }
        }
    }

    {
        let mut t = TESSERACT.lock();
        t.log_system("genesis complete");
        if cfg.agent_enabled {
            t.log_system(format!("storage-agent online: {}", cfg.agent_label));
        }
        t.log_system("type help to list intents");
        t.dirty = true;
    }

    {
        let f = FABRIC.lock();
        if let Some(w) = fb.lock().as_mut() {
            render(w, &f, &TESSERACT.lock());
        }
        shim.present_frame();
    }
    {
        let mut t = TESSERACT.lock();
        t.dirty = false;
    }

    loop {
        // 1. Observe — drain input bytes from the substrate.
        loop {
            match shim.execute(Op::PollInput) {
                OpResult::Input(Some(b)) => editor.push_byte(b),
                _ => break,
            }
        }
        if let Some(line) = editor.poll() {
            let exchange = {
                let mut f = FABRIC.lock();
                intent::submit(&mut f, &line)
            };
            {
                let mut t = TESSERACT.lock();
                t.log_operator(line.clone());
                for resp_line in exchange.response_text.lines() {
                    t.log_fabric(resp_line);
                }
                t.current_input.clear();
                t.dirty = true;
            }
            intent_pending_persist = true;
        }

        let current = editor.current().to_string();
        if current != last_input_render {
            let mut t = TESSERACT.lock();
            t.current_input = current.clone();
            t.dirty = true;
            last_input_render = current;
        }

        // 2. Active inference over the storage controller.
        let now_ticks = match shim.execute(Op::GetTime) {
            OpResult::Time(t) => t,
            _ => 0,
        };
        if cfg.agent_enabled
            && now_ticks.saturating_sub(last_agent_tick) >= AGENT_STEP_EVERY_TICK
        {
            let report_and_summary: Option<(crate::storage_agent::StepReport, alloc::string::String)> = {
                let mut slot = MODEL.lock();
                slot.as_mut().and_then(|model| {
                    let report = {
                        let d = model.devices.storage_mut()?;
                        d.agent.step(shim)
                    };
                    let summary = model
                        .devices
                        .storage()
                        .map(|d| d.render_summary())
                        .unwrap_or_default();
                    let action_label =
                        crate::storage_agent::action_label(report.action).to_string();
                    let state_label =
                        crate::storage_agent::state_label(report.map_state).to_string();
                    let surprise = report.surprise;
                    let note = format!(
                        "{}/{}/{}",
                        action_label,
                        obs_label(report.observation),
                        state_label
                    );

                    // Causal graph: action → outcome edge.
                    let action_id = model.causal_graph.intern(
                        &format!("storage-action:{}", action_label),
                        "devices",
                    );
                    let outcome_id = model.causal_graph.intern(
                        &format!("storage-obs:{}", obs_label(report.observation)),
                        "devices",
                    );
                    model.causal_graph.record(action_id, outcome_id);

                    model.account_observation("devices", surprise, note);
                    Some((report, summary))
                })
            };

            if let Some((report, summary)) = report_and_summary {
                last_agent_tick = now_ticks;

                let event_text = format!(
                    "storage: action={} obs={} state={} surp={:.2}",
                    crate::storage_agent::action_label(report.action),
                    obs_label(report.observation),
                    crate::storage_agent::state_label(report.map_state),
                    report.surprise,
                );

                {
                    let mut f = FABRIC.lock();
                    let lamport = f.lamport;
                    f.create(NodeKind::SystemEvent {
                        text: event_text,
                        lamport,
                    });
                }

                {
                    let mut t = TESSERACT.lock();
                    t.set_storage_agent_summary(summary);
                    t.dirty = true;
                }
            }
        }

        // 3. Decay — small per-cycle weight decay so node weights drift.
        {
            let mut f = FABRIC.lock();
            if f.lamport % 64 == 0 {
                f.decay(0.999);
            }
        }

        // 4. Meta-region self-assessment.
        let lamport_now = FABRIC.lock().lamport;
        if lamport_now.saturating_sub(last_meta_lamport) >= META_ASSESS_EVERY_LAMPORT {
            last_meta_lamport = lamport_now;
            let assessment_text = {
                let mut slot = MODEL.lock();
                slot.as_mut().map(|model| {
                    model.lamport = lamport_now;
                    let delta = model.history.delta_fe().unwrap_or(0.0);
                    let current = model.history.current_fe().unwrap_or(0.0);
                    let a = model.meta.assess(delta, current);
                    a.render()
                })
            };
            if let Some(text) = assessment_text {
                let mut f = FABRIC.lock();
                let lamport = f.lamport;
                f.create(NodeKind::SystemEvent {
                    text,
                    lamport,
                });
            }
        }

        // 5. Render.
        let dirty = TESSERACT.lock().dirty;
        if dirty {
            let f = FABRIC.lock();
            let t = TESSERACT.lock();
            if let Some(w) = fb.lock().as_mut() {
                render(w, &f, &t);
            }
            drop(t);
            drop(f);
            TESSERACT.lock().dirty = false;
            shim.present_frame();
        }

        // 6. Persist (via the shim).
        if cfg.persist_enabled {
            let lamport = FABRIC.lock().lamport;
            if last_persist_attempt == u64::MAX {
                last_persist_attempt = lamport;
            }
            let due = lamport.saturating_sub(last_persist_attempt) >= PERSIST_EVERY_LAMPORT;
            let explicit = intent::PERSIST_REQUESTED
                .swap(false, core::sync::atomic::Ordering::AcqRel);
            if intent_pending_persist || due || explicit {
                // Step 7 — full-model snapshot lives as a LearnedDriver
                // node with kind="model"; the per-device kind="storage"
                // node is preserved as a backward-compatibility shim
                // for older images and for emergency restore when the
                // full-model deserialise fails.
                let model_blob = {
                    let slot = MODEL.lock();
                    slot.as_ref().map(|m| {
                        let bytes = m.serialize_to_bytes();
                        let obs = m.total_observations;
                        let avg = (m.average_surprise() * 1000.0) as u32;
                        (bytes, obs, avg)
                    })
                };
                let driver_node = {
                    let slot = MODEL.lock();
                    slot.as_ref().and_then(|model| {
                        model.devices.storage().map(|d| {
                            let bytes = d.snapshot_bytes();
                            let avg = (d.average_surprise() * 1000.0) as u32;
                            (bytes, d.observations_seen(), avg)
                        })
                    })
                };
                if let Some((bytes, obs, avg)) = model_blob {
                    let mut f = FABRIC.lock();
                    f.create(NodeKind::LearnedDriver {
                        kind: "model".into(),
                        observations: obs,
                        avg_surprise_x1000: avg,
                        params: bytes,
                    });
                }
                if let Some((bytes, obs, avg)) = driver_node {
                    let mut f = FABRIC.lock();
                    f.create(NodeKind::LearnedDriver {
                        kind: "storage".into(),
                        observations: obs,
                        avg_surprise_x1000: avg,
                        params: bytes,
                    });
                }
                let result = {
                    let f = FABRIC.lock();
                    snapshot::persist(&f, shim)
                };
                match result {
                    Ok(bytes) => {
                        let mut slot = MODEL.lock();
                        if let Some(m) = slot.as_mut() {
                            m.persistence.note_persist_outcome(true);
                        }
                        TESSERACT.lock().log_system(format!("persisted {} bytes", bytes));
                    }
                    Err(e) => {
                        let mut slot = MODEL.lock();
                        if let Some(m) = slot.as_mut() {
                            m.persistence.note_persist_outcome(false);
                        }
                        TESSERACT.lock().log_warning(format!("persist failed: {}", e));
                    }
                }
                last_persist_attempt = lamport;
                intent_pending_persist = false;
            }
        }

        // 7. Park CPU.
        shim.execute(Op::Halt);
    }
}


fn obs_label(o: usize) -> &'static str {
    use crate::storage_agent;
    match o {
        storage_agent::OBS_OK_FAST => "ok-fast",
        storage_agent::OBS_OK_SLOW => "ok-slow",
        storage_agent::OBS_DRQ => "drq",
        storage_agent::OBS_TIMEOUT => "timeout",
        storage_agent::OBS_DEV_ERROR => "dev-err",
        _ => "?",
    }
}

/// Hand a single byte stream to the substrate-agnostic line editor —
/// used by the input keyboard layer in inference loop's `PollInput`
/// integration.
pub fn _editor_byte_consumer() {}
