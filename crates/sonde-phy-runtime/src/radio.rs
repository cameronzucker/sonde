//! The half-duplex hardware seam.
//!
//! [`Radio`] hides whether the modem is talking to a real soundcard+PTT or an
//! in-memory loop. The runtime is half-duplex: a [`Radio::transmit`] call owns
//! the channel for its duration (PTT keyed); [`Radio::receive`] captures a
//! window only while not transmitting. This mirrors a single SSB rig + one
//! soundcard, where TX and RX cannot overlap.

use sonde_phy::error::PhyError;

/// A half-duplex audio radio. Implementors handle their own PTT inside
/// [`Radio::transmit`] (assert before the first sample, release after the
/// last). `Send` so the runtime worker can own one.
pub trait Radio: Send {
    /// Transmit `samples` end-to-end (PTT lead-in → play → tail-drain →
    /// release). Blocks until the air is clear.
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError>;

    /// Capture up to `max_samples` of receive audio. Returns at least one
    /// sample; may return fewer than `max_samples` if the capture window closes
    /// early. Never keys PTT.
    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError>;
}

/// In-memory half-duplex radio for hardware-free tests. Whatever is
/// `transmit`ted is buffered and handed back, wrapped in leading + trailing
/// silence, on the next `receive` — modelling a perfect channel that loops the
/// transmitter into the receiver. No fading, no noise: that is the channel
/// simulator's job (a separate plan), not this double's.
pub struct LoopbackRadio {
    pending: Vec<f32>,
    lead_silence: usize,
}

impl LoopbackRadio {
    /// New loopback radio with a default 200-sample leading silence so decoders
    /// must exercise their preamble search at a non-zero offset.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            lead_silence: 200,
        }
    }
}

impl Default for LoopbackRadio {
    fn default() -> Self {
        Self::new()
    }
}

impl Radio for LoopbackRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
        let mut framed = vec![0.0f32; self.lead_silence];
        framed.extend_from_slice(samples);
        framed.extend(std::iter::repeat(0.0).take(self.lead_silence));
        self.pending = framed;
        Ok(())
    }

    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError> {
        if self.pending.is_empty() {
            return Ok(vec![0.0f32; max_samples]);
        }
        let out = std::mem::take(&mut self.pending);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_returns_transmitted_samples_on_next_receive() {
        let mut radio = LoopbackRadio::new();
        let tx = vec![0.1, 0.2, 0.3, -0.4];
        radio.transmit(&tx).expect("transmit");

        let rx = radio.receive(1024).expect("receive");
        // Loopback wraps the TX with the same leading/trailing silence a real
        // capture window would: the TX samples must appear verbatim somewhere
        // inside the returned window.
        assert!(
            rx.windows(tx.len()).any(|w| w == tx.as_slice()),
            "transmitted samples must round-trip through the loopback"
        );
    }

    #[test]
    fn loopback_receive_is_silence_when_nothing_was_transmitted() {
        let mut radio = LoopbackRadio::new();
        let rx = radio.receive(256).expect("receive");
        assert_eq!(rx.len(), 256);
        assert!(rx.iter().all(|&s| s == 0.0));
    }
}
