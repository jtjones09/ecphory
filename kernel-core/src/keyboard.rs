//! Substrate-agnostic line editor.
//!
//! The substrate's shim feeds us cooked bytes through `Op::PollInput` —
//! ASCII / Unicode, not raw scancodes. Per-arch shims handle scancode
//! translation (PS/2 set-1 on x86, USB-HID on aarch64 when wired up).
//! That keeps the encoding-specific code at the substrate boundary,
//! and the line-editing logic substrate-agnostic.

use alloc::string::String;

const LINE_CAP: usize = 256;

pub struct LineEditor {
    line: String,
    pending: alloc::collections::VecDeque<u8>,
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            line: String::new(),
            pending: alloc::collections::VecDeque::new(),
        }
    }

    pub fn current(&self) -> &str {
        &self.line
    }

    pub fn push_byte(&mut self, b: u8) {
        self.pending.push_back(b);
    }

    pub fn poll(&mut self) -> Option<String> {
        while let Some(b) = self.pending.pop_front() {
            match b {
                b'\n' | b'\r' => {
                    let line = core::mem::take(&mut self.line);
                    return Some(line);
                }
                0x08 | 0x7F => {
                    self.line.pop();
                }
                b if b.is_ascii_control() => {}
                b => {
                    if self.line.len() < LINE_CAP {
                        self.line.push(b as char);
                    }
                }
            }
        }
        None
    }
}
