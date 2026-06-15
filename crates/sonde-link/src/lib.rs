//! sonde-link — the modem-owned Link layer (#5), connected-mode selective-repeat
//! ARQ (#6), and a minimal host surface (#8), built on the `PhyTransport` seam.
//!
//! # Operating model: half-duplex, push-to-talk, turn-taking
//!
//! An amateur HF data link in the VARA/ARDOP *vein* — **not** a packet network.
//! One rig, one channel, half-duplex at best: a station transmits **or** receives,
//! never both, and is deaf while keyed. Only one station transmits at a time;
//! overlapping transmissions collide. Turnaround (drop PTT → peer keys up) is
//! explicit. Selective-repeat ARQ here is window-**per-over**, turn-taking — never
//! full-duplex pipelining.
//!
//! # Discipline
//!
//! No capability is "done" until a gate proves it. This layer is validated by
//! reliable in-order delivery over a *realistic* lossy channel (bursty loss +
//! corruption) plus connection establish/teardown — see the crate's design doc
//! and the `tests/` harness. Results are "link-correct over a channel model,"
//! never "HF-viable": over-the-real-PHY viability is integration work gated on the
//! PHY physics gates.
//!
//! Per RADIO-1 / Part 97, nothing here keys a real radio; the link is exercised
//! against in-memory `PhyTransport`/`Radio` doubles.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod arq;
pub mod clock;
pub mod conn;
pub mod driver;
pub mod frame;
pub mod host;
pub mod link;
pub mod mac;
pub mod profile;

pub use clock::{Clock, SystemClock};
pub use conn::{ConnState, Connection, HostEvent};
pub use driver::Driver;
pub use frame::{Callsign, FrameError, FrameType, LinkFrame, LINK_MTU, LINK_OVERHEAD};
pub use host::HostCommand;
pub use link::Link;
pub use mac::{
    adapt_rung, base_rung, clamp_available, default_rung, recommended_rung, route, rung,
    ArqStrategy, Route, WindowParams, NUM_RUNGS,
};
pub use profile::ModeProfile;
