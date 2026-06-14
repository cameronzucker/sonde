//! Production `PhyTransport` runtime for the sonde modem.
//!
//! [`SondePhy`] is the first real implementation of
//! [`sonde_phy::phy_api::PhyTransport`] (the `NullPhy` in `sonde-phy` is a
//! loopback test fixture). It lets the Tuxlink link/MAC (#5) and
//! link-adaptation (#7) layers drive a real modem through the documented
//! inter-subsystem boundary.
//!
//! # Two seams
//!
//! - [`Waveform`] — modulation/demodulation. [`FloorWaveform`] wraps the HF
//!   wide-band low-density floor today. **An FM variant ("Sonde FM", VHF/UHF)
//!   is added by implementing [`Waveform`] for an `FmWaveform` and registering
//!   it under [`sonde_phy::modes::ModeFamily`]** — see the crate README's "FM
//!   extension point". The trait deliberately carries no HF-only (SSB-passband)
//!   assumptions.
//! - [`Radio`] — half-duplex hardware. [`LoopbackRadio`] is the hardware-free
//!   test double; the production `SoundcardRadio` (feature `hardware`) composes
//!   the soundcard + PTT.
//!
//! Per RADIO-1, an agent never runs the production path against a real radio;
//! the operator (licensee) does.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod radio;
pub mod runtime;
pub mod waveform;

#[cfg(feature = "hardware")]
pub mod soundcard;

pub use radio::{LoopbackRadio, Radio};
pub use runtime::SondePhy;
pub use waveform::{DecodedFrame, FloorWaveform, Waveform};

#[cfg(feature = "hardware")]
pub use soundcard::SoundcardRadio;
