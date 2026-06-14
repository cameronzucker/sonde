//! Production half-duplex `Radio` over a CPAL soundcard + serial-RTS PTT.
//!
//! NOT exercised by automated tests — RADIO-1 forbids an agent keying a real
//! transmitter. The operator (licensee) runs this. It is kept thin: all PTT
//! timing / airtime safety lives in `sonde-tx`'s `run_transmission`, which this
//! delegates to.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use sonde_phy::audio_device::{AudioInput, AudioOutput};
use sonde_phy::audio_io::AudioBuffer;
use sonde_phy::error::PhyError;
use sonde_rig_rts::{LinuxTty, RtsPtt};
use sonde_tx::{check_budget, run_transmission, AirtimeBudget, DEFAULT_LEAD_IN};

use crate::radio::Radio;

/// Soundcard + RTS-PTT production radio.
pub struct SoundcardRadio {
    output: AudioOutput,
    input: AudioInput,
    ptt: RtsPtt<LinuxTty>,
    max_airtime: Duration,
}

impl SoundcardRadio {
    /// Open the named output + input devices and the RTS PTT on `tty`. `None`
    /// device names select the system default. `max_airtime` caps a single
    /// transmission (the airtime-budget gate rejects longer buffers before PTT).
    pub fn open(
        output_device: Option<&str>,
        input_device: Option<&str>,
        tty: &Path,
        max_airtime: Duration,
    ) -> Result<Self, PhyError> {
        let output = AudioOutput::open(output_device)?;
        let input = AudioInput::open(input_device)?;
        let linux_tty =
            LinuxTty::open(tty).map_err(|e| PhyError::AudioIo(format!("open PTT tty: {e}")))?;
        let ptt =
            RtsPtt::new(linux_tty).map_err(|e| PhyError::AudioIo(format!("init PTT: {e}")))?;
        Ok(Self {
            output,
            input,
            ptt,
            max_airtime,
        })
    }
}

impl Radio for SoundcardRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
        let buffer = AudioBuffer::from_samples(samples.to_vec());
        let budget = AirtimeBudget::from_buffer_defaults(&buffer);
        check_budget(&budget, self.max_airtime)
            .map_err(|e| PhyError::AudioIo(format!("airtime budget: {e}")))?;
        let abort = AtomicBool::new(false);
        run_transmission(
            &mut self.ptt,
            &mut self.output,
            &buffer,
            DEFAULT_LEAD_IN,
            &abort,
        )
        .map_err(|e| PhyError::AudioIo(format!("transmit: {e}")))?;
        Ok(())
    }

    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError> {
        // Capture exactly one RX window; both Completed and Aborted outcomes
        // yield whatever was captured.
        let abort = AtomicBool::new(false);
        let (_outcome, buffer) = self.input.record_blocking_with_abort(max_samples, &abort)?;
        Ok(buffer.into_samples())
    }
}
