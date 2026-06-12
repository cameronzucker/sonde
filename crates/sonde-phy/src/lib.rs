//! sonde-phy — clean-sheet HF PHY waveform layer.
//!
//! Subordinate to `docs/superpowers/specs/2026-05-31-clean-sheet-modem-3-phy-waveform.md`
//! in the tuxlink repo. No examination of VARA / ARDOP / FLDigi / Trimode /
//! Pat / wl2k-go internals (ADR 0014).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "audio-device")]
pub mod audio_device;
pub mod audio_io;
pub mod coded_modulation;
pub mod constellations;
pub mod error;
pub mod modes;
pub mod ofdm_main;
pub mod phy_api;
pub mod robustness_floor;
pub mod subcarrier_snr;
pub mod sync;
pub use error::PhyError;
