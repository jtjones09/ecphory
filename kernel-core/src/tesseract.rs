//! The Tesseract — observation surface that adapts to whoever is
//! looking. Phase 1 v1: text-mode, two panes, colour-coded.
//!
//! Left pane: the fabric's current world model (counts, highlights).
//! Right pane: the interaction log (operator intents and fabric replies).

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};

use crate::fabric::{Fabric, NodeKind};
use crate::framebuffer::{
    BLUE, CHAR_HEIGHT, CHAR_WIDTH, CYAN, Color, DIM, FrameBufferWriter, GREEN, LETTER_SPACING,
    LINE_SPACING, RED, WHITE, YELLOW,
};

const BORDER: usize = 12;
const PANE_GAP: usize = 24;
const HEADER_HEIGHT: usize = CHAR_HEIGHT + 4;
const LINE_STEP: usize = CHAR_HEIGHT + LINE_SPACING;
const PROMPT_HEIGHT: usize = CHAR_HEIGHT + 8;
const LOG_CAP: usize = 200;

#[derive(Clone, Copy, Debug)]
pub enum LogKind {
    System,
    Operator,
    Fabric,
    Warning,
}

#[derive(Clone)]
pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
}

pub struct Tesseract {
    pub log: VecDeque<LogLine>,
    pub current_input: String,
    pub last_persist_lamport: u64,
    pub dirty: bool,
    pub storage_agent_summary: String,
}

impl Tesseract {
    pub const fn new() -> Self {
        Self {
            log: VecDeque::new(),
            current_input: String::new(),
            last_persist_lamport: 0,
            dirty: true,
            storage_agent_summary: String::new(),
        }
    }

    pub fn set_storage_agent_summary(&mut self, s: impl Into<String>) {
        self.storage_agent_summary = s.into();
    }

    pub fn log_system(&mut self, text: impl Into<String>) {
        self.push(LogLine {
            kind: LogKind::System,
            text: text.into(),
        });
    }

    pub fn log_operator(&mut self, text: impl Into<String>) {
        self.push(LogLine {
            kind: LogKind::Operator,
            text: text.into(),
        });
    }

    pub fn log_fabric(&mut self, text: impl Into<String>) {
        self.push(LogLine {
            kind: LogKind::Fabric,
            text: text.into(),
        });
    }

    pub fn log_warning(&mut self, text: impl Into<String>) {
        self.push(LogLine {
            kind: LogKind::Warning,
            text: text.into(),
        });
    }

    fn push(&mut self, line: LogLine) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(line);
        self.dirty = true;
    }
}

pub static TESSERACT: spin::Mutex<Tesseract> = spin::Mutex::new(Tesseract::new());

pub fn render(fb: &mut FrameBufferWriter, fabric: &Fabric, tess: &Tesseract) {
    let info = fb.info();
    let width = info.width;
    let height = info.height;

    // Clear with a near-black so dim text stays legible.
    fb.fill_rect(0, 0, width, height, Color(8, 10, 14));

    let pane_top = BORDER;
    let pane_bottom = height - PROMPT_HEIGHT - BORDER;
    let split = width / 2;
    let left_x = BORDER;
    let left_w = split - BORDER - PANE_GAP / 2;
    let right_x = split + PANE_GAP / 2;
    let right_w = width - right_x - BORDER;
    let pane_h = pane_bottom - pane_top;

    // Pane separators.
    fb.draw_vline(split, pane_top, pane_h, Color(40, 60, 80));
    fb.draw_hline(0, pane_bottom, width, Color(40, 60, 80));

    render_state(fb, fabric, left_x, pane_top, left_w, pane_h);
    render_log(fb, &tess.log, right_x, pane_top, right_w, pane_h);
    render_prompt(fb, &tess.current_input, fabric, BORDER, pane_bottom + 8);

    if !tess.storage_agent_summary.is_empty() {
        let agent_y = pane_top + (pane_h * 3) / 4;
        fb.draw_text(left_x, agent_y, "STORAGE AGENT", BLUE);
        let mut row = agent_y + HEADER_HEIGHT + 4;
        fb.draw_hline(left_x, row, left_w, Color(40, 50, 70));
        row += LINE_SPACING + 4;
        for line in FrameBufferWriter::wrap_lines(&tess.storage_agent_summary, left_w) {
            fb.draw_text(left_x, row, line, GREEN);
            row += LINE_STEP;
        }
    }
}

fn render_state(
    fb: &mut FrameBufferWriter,
    f: &Fabric,
    x: usize,
    y: usize,
    w: usize,
    _h: usize,
) {
    let mut row_y = y;
    fb.draw_text(x, row_y, "FABRIC STATE", BLUE);
    row_y += HEADER_HEIGHT + 4;
    fb.draw_hline(x, row_y, w, Color(40, 50, 70));
    row_y += LINE_SPACING + 4;

    let put = |fb: &mut FrameBufferWriter, label: &str, value: &str, color: Color, ry: &mut usize| {
        fb.draw_text(x, *ry, label, DIM);
        let value_x = x + 11 * (CHAR_WIDTH + LETTER_SPACING);
        fb.draw_text(value_x, *ry, value, color);
        *ry += LINE_STEP;
    };

    put(fb, "lamport  :", &format!("{}", f.lamport), WHITE, &mut row_y);
    put(fb, "nodes    :", &format!("{}", f.nodes.len()), WHITE, &mut row_y);
    put(fb, "edges    :", &format!("{}", f.edges.len()), WHITE, &mut row_y);

    row_y += 4;

    // CPU
    if let Some(cpu) = f.iter_kind(1).next() {
        if let NodeKind::HwCpu { vendor, brand } = &cpu.kind {
            put(fb, "cpu      :", vendor, CYAN, &mut row_y);
            // brand wrapped
            let brand_color = WHITE;
            for line in FrameBufferWriter::wrap_lines(brand, w) {
                fb.draw_text(x + 11 * (CHAR_WIDTH + LETTER_SPACING), row_y, line, brand_color);
                row_y += LINE_STEP;
            }
        }
    }

    let features = f.count_by_tag(2);
    put(fb, "features :", &format!("{}", features), WHITE, &mut row_y);
    let memory = f.count_by_tag(3);
    let usable: u64 = f
        .iter_kind(3)
        .filter_map(|n| match &n.kind {
            NodeKind::HwMemoryRegion { start, end, kind } if kind == "usable" => {
                Some(end - start)
            }
            _ => None,
        })
        .sum();
    put(
        fb,
        "memory   :",
        &format!("{} regions, {} MiB usable", memory, usable / (1024 * 1024)),
        WHITE,
        &mut row_y,
    );
    let pci = f.count_by_tag(4);
    put(fb, "pci dev  :", &format!("{}", pci), WHITE, &mut row_y);
    let acpi = f.count_by_tag(5);
    put(fb, "acpi tab :", &format!("{}", acpi), WHITE, &mut row_y);
    let storage = f.count_by_tag(7);
    put(fb, "storage  :", &format!("{}", storage), WHITE, &mut row_y);
    let intents = f.count_by_tag(8);
    put(fb, "intents  :", &format!("{}", intents), YELLOW, &mut row_y);
    let responses = f.count_by_tag(9);
    put(
        fb,
        "responses:",
        &format!("{}", responses),
        CYAN,
        &mut row_y,
    );

    row_y += 8;

    // Health: average weight across all nodes.
    let total: f32 = f.nodes.iter().map(|n| n.weight).sum();
    let avg = if f.nodes.is_empty() {
        0.0
    } else {
        total / f.nodes.len() as f32
    };
    let (label, color) = if avg > 0.7 {
        ("green", GREEN)
    } else if avg > 0.3 {
        ("yellow", YELLOW)
    } else {
        ("red", RED)
    };
    put(
        fb,
        "immune   :",
        &format!("{} (avg w {:.2})", label, avg),
        color,
        &mut row_y,
    );
}

fn render_log(
    fb: &mut FrameBufferWriter,
    log: &VecDeque<LogLine>,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let mut row_y = y;
    fb.draw_text(x, row_y, "INTERACTION LOG", BLUE);
    row_y += HEADER_HEIGHT + 4;
    fb.draw_hline(x, row_y, w, Color(40, 50, 70));
    row_y += LINE_SPACING + 4;

    let max_lines = (h.saturating_sub(row_y - y)) / LINE_STEP;
    // collect wrapped lines, then take the last `max_lines` so newest stays visible.
    let mut all_lines: alloc::vec::Vec<(LogKind, String)> = alloc::vec::Vec::new();
    for entry in log {
        let prefix = match entry.kind {
            LogKind::Operator => "> ",
            LogKind::Fabric => "< ",
            LogKind::System => "~ ",
            LogKind::Warning => "! ",
        };
        let combined = format!("{}{}", prefix, entry.text);
        for line in FrameBufferWriter::wrap_lines(&combined, w) {
            all_lines.push((entry.kind, line.to_string()));
        }
    }
    let skip = all_lines.len().saturating_sub(max_lines);
    for (kind, line) in all_lines.iter().skip(skip) {
        let color = match kind {
            LogKind::Operator => YELLOW,
            LogKind::Fabric => CYAN,
            LogKind::System => GREEN,
            LogKind::Warning => RED,
        };
        fb.draw_text(x, row_y, line, color);
        row_y += LINE_STEP;
    }
}

fn render_prompt(fb: &mut FrameBufferWriter, current: &str, _f: &Fabric, x: usize, y: usize) {
    fb.draw_text(x, y, "operator >", BLUE);
    let cursor_x = x + 11 * (CHAR_WIDTH + LETTER_SPACING);
    fb.draw_text(cursor_x, y, current, WHITE);
    // Blinking-style cursor: just always show.
    let cur_x = cursor_x + current.chars().count() * (CHAR_WIDTH + LETTER_SPACING);
    fb.fill_rect(cur_x, y + 2, 2, CHAR_HEIGHT, WHITE);
}
