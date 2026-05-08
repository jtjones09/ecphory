//! The active-inference main loop. Substrate-independent: takes a
//! `&mut dyn Shim` and drives observe → act → render → persist forever.
//!
//! Compared to Phase 1, the Phase 2 loop also runs hardware
//! [`StorageAgent`]s — discrete active-inference agents that learn the
//! storage controller's behavioural patterns. Each agent step adds
//! observed-latency / action-taken / state-belief into the fabric so
//! the immune system and Tesseract see the controller as a learnable
//! object, not a black box behind a static driver.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fabric::FABRIC;
use crate::framebuffer::FrameBufferWriter;
use crate::keyboard::LineEditor;
use crate::ops::{Op, OpResult, Shim};
use crate::intent;
use crate::snapshot;
use crate::storage_agent::StorageAgent;
use crate::tesseract::{TESSERACT, render};
use crate::NodeKind;

/// How often the storage agent runs one full step. Keeping it
/// throttled — every Nth iteration — leaves headroom for input
/// latency.
const AGENT_STEP_EVERY_TICK: u64 = 16;
const PERSIST_EVERY_LAMPORT: u64 = 5;

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
    let mut intent_pending_persist = false;

    // Storage agent: only active if the substrate has storage.
    let mut storage_agent: Option<StorageAgent> = if cfg.agent_enabled {
        let mut a = StorageAgent::new(cfg.agent_label.clone());

        // 1. Try to restore the agent's learned A/B from the fabric
        //    snapshot. We pick the LearnedDriver of kind="storage"
        //    with the highest created_at lamport.
        let restored = {
            let f = FABRIC.lock();
            let mut latest: Option<(u64, alloc::vec::Vec<u8>, u64, u32)> = None;
            for n in f.iter_kind(11) {
                if let NodeKind::LearnedDriver {
                    kind,
                    observations,
                    avg_surprise_x1000,
                    params,
                } = &n.kind
                {
                    if kind == "storage" {
                        let take = match latest {
                            None => true,
                            Some((c, _, _, _)) => n.created_at > c,
                        };
                        if take {
                            latest = Some((
                                n.created_at,
                                params.clone(),
                                *observations,
                                *avg_surprise_x1000,
                            ));
                        }
                    }
                }
            }
            latest
        };
        let mut continued = false;
        if let Some((_, params, _obs, _surp)) = restored {
            if a.restore_from_bytes(&params) {
                continued = true;
            }
        }

        {
            let mut f = FABRIC.lock();
            let lamport = f.lamport;
            let label = a.label.clone();
            f.create(NodeKind::SystemEvent {
                text: if continued {
                    format!(
                        "storage-agent: continued for {} (obs={} avg_surp={:.3})",
                        label,
                        a.model.observations_seen,
                        a.model.average_surprise()
                    )
                } else {
                    format!("storage-agent: spawned fresh for {}", label)
                },
                lamport,
            });
        }
        // Run a quick initial probe so the median-latency is non-zero.
        let _ = a.step(shim);
        Some(a)
    } else {
        None
    };

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
        // 1. Observe — drain input bytes from the substrate, hand them
        //    to the line editor.
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

        // 2. Active inference over the storage controller — every Nth
        //    iteration so the kernel mostly sleeps between probes.
        let now_ticks = match shim.execute(Op::GetTime) {
            OpResult::Time(t) => t,
            _ => 0,
        };
        if let Some(agent) = storage_agent.as_mut() {
            if now_ticks.saturating_sub(last_agent_tick) >= AGENT_STEP_EVERY_TICK {
                let report = agent.step(shim);
                last_agent_tick = now_ticks;

                // Surface the step to the fabric so the immune system
                // sees behavioural data, and to the Tesseract so the
                // operator sees what's happening.
                {
                    let mut f = FABRIC.lock();
                    let lamport = f.lamport;
                    f.create(NodeKind::SystemEvent {
                        text: format!(
                            "storage: action={} obs={} state={} surp={:.2}",
                            crate::storage_agent::action_label(report.action),
                            obs_label(report.observation),
                            crate::storage_agent::state_label(report.map_state),
                            report.surprise,
                        ),
                        lamport,
                    });
                }

                {
                    let mut t = TESSERACT.lock();
                    t.set_storage_agent_summary(agent.render_summary());
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

        // 4. Render.
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

        // 5. Persist (via the shim).
        if cfg.persist_enabled {
            let lamport = FABRIC.lock().lamport;
            if last_persist_attempt == u64::MAX {
                last_persist_attempt = lamport;
            }
            let due = lamport.saturating_sub(last_persist_attempt) >= PERSIST_EVERY_LAMPORT;
            let explicit = intent::PERSIST_REQUESTED
                .swap(false, core::sync::atomic::Ordering::AcqRel);
            if intent_pending_persist || due || explicit {
                // Write the agent's current matrices into the fabric so
                // the next boot can resume from learned state.
                if let Some(agent) = storage_agent.as_ref() {
                    let bytes = agent.snapshot_bytes();
                    let avg = (agent.model.average_surprise() * 1000.0) as u32;
                    let mut f = FABRIC.lock();
                    f.create(NodeKind::LearnedDriver {
                        kind: "storage".into(),
                        observations: agent.model.observations_seen,
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
                        TESSERACT.lock().log_system(format!("persisted {} bytes", bytes));
                    }
                    Err(e) => {
                        TESSERACT.lock().log_warning(format!("persist failed: {}", e));
                    }
                }
                last_persist_attempt = lamport;
                intent_pending_persist = false;
            }
        }

        // 6. Park CPU.
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
