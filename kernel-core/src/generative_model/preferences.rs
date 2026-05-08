//! C-vector preferences — the four-questions ln-preferences that shape
//! action selection through the pragmatic term in expected free energy.
//!
//! These are NOT post-hoc filters. They live inside each region's
//! `DiscreteModel::c` so the agent's action search naturally avoids
//! observations the kernel finds dispreferred (corruption, nonresponse,
//! discontinuity, information_loss).

use alloc::vec::Vec;

/// The four questions, expressed numerically. Higher = preferred.
///
/// Per `nisaba/positions/nucleation-architecture.md` the four questions
/// are: am I healthy, do I have agency, am I consistent, do I have
/// integrity. Each region maps these onto its own observation space
/// when it builds its C-vector at nucleation time.
#[derive(Clone, Debug)]
pub struct Preferences {
    pub health: f32,
    pub agency: f32,
    pub consistency: f32,
    pub integrity: f32,
    pub corruption: f32,
    pub nonresponse: f32,
    pub discontinuity: f32,
    pub information_loss: f32,
}

impl Preferences {
    pub const fn nucleation() -> Self {
        Self {
            health: 2.0,
            agency: 1.5,
            consistency: 1.0,
            integrity: 2.0,
            corruption: -4.0,
            nonresponse: -3.0,
            discontinuity: -1.5,
            information_loss: -3.5,
        }
    }

    pub fn serialize(&self, out: &mut Vec<u8>) {
        for &v in &[
            self.health,
            self.agency,
            self.consistency,
            self.integrity,
            self.corruption,
            self.nonresponse,
            self.discontinuity,
            self.information_loss,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    pub fn deserialize(bytes: &[u8], off: &mut usize) -> Option<Self> {
        if *off + 32 > bytes.len() {
            return None;
        }
        let mut read = || {
            let v = f32::from_le_bytes(bytes[*off..*off + 4].try_into().ok()?);
            *off += 4;
            Some(v)
        };
        Some(Self {
            health: read()?,
            agency: read()?,
            consistency: read()?,
            integrity: read()?,
            corruption: read()?,
            nonresponse: read()?,
            discontinuity: read()?,
            information_loss: read()?,
        })
    }
}
