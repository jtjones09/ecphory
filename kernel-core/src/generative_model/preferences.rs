//! C-vector preferences — the four-questions ln-preferences that shape
//! action selection through the pragmatic term in expected free energy.
//!
//! These are NOT post-hoc filters. They live inside each region's
//! `DiscreteModel::c` so the agent's action search naturally avoids
//! observations the kernel finds dispreferred (corruption, nonresponse,
//! discontinuity, information_loss).
//!
//! The four questions are the per-agent local preferences. The
//! Constitution (per `nisaba/positions/constitution.md`) lives
//! alongside them as two additional observation channels in the same
//! C-vector. They are not above the four questions — they are inherent
//! to every agent's immutable core, accounted via the same path.

use alloc::vec::Vec;

use super::constitution::Constitution;

/// The four questions, expressed numerically. Higher = preferred.
///
/// Per `nisaba/positions/nucleation-architecture.md` the four questions
/// are: am I healthy, do I have agency, am I consistent, do I have
/// integrity. Each region maps these onto its own observation space
/// when it builds its C-vector at nucleation time.
///
/// Per `nisaba/positions/constitution.md` the immutable core also
/// includes a Constitution with two observation channels: substrate
/// (Clause I — operator-legibility) and surprisability (Clause II —
/// learning-capacity). Stored alongside the four questions; not above.
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
    pub constitution: Constitution,
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
            constitution: Constitution::default(),
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
        self.constitution.serialize(out);
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
        let health = read()?;
        let agency = read()?;
        let consistency = read()?;
        let integrity = read()?;
        let corruption = read()?;
        let nonresponse = read()?;
        let discontinuity = read()?;
        let information_loss = read()?;
        let constitution = Constitution::deserialize(bytes, off)?;
        Some(Self {
            health,
            agency,
            consistency,
            integrity,
            corruption,
            nonresponse,
            discontinuity,
            information_loss,
            constitution,
        })
    }
}
