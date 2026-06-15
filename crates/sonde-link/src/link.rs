//! `Link<P>` — the thin adapter that drives the sans-IO [`Connection`] over any
//! `P: PhyTransport`.
//!
//! The connection state machine holds all the protocol logic and is fully
//! testable in isolation; this adapter only does I/O plumbing: serialize
//! outbound [`LinkFrame`]s to `send_frame`, decode inbound `poll_rx` bytes back
//! into frames (a failed CRC/decode is simply dropped — exactly the "remote
//! loss inferred from a missing reply" model of the seam), run the clock, and
//! surface host events. Being generic over `P` means the same driver runs over
//! the in-memory lossy medium (gates G1–G4) and a real `SondePhy` (gate G5)
//! unchanged.

use std::time::Duration;

use sonde_phy::phy_api::PhyTransport;

use crate::conn::{ConnState, Connection, HostEvent};
use crate::frame::LinkFrame;
use crate::host::HostCommand;

/// Drives a [`Connection`] over a `PhyTransport`.
pub struct Link<P: PhyTransport> {
    conn: Connection,
    phy: P,
}

impl<P: PhyTransport> Link<P> {
    /// Wrap a connection and a PHY/transport. Frames are transmitted at the
    /// connection's *current* mode ([`Connection::current_hint`]) — like the
    /// [`Driver`](crate::Driver) — so a mid-session mode change takes effect on the
    /// wire automatically (when the PHY exposes >1 waveform; see sonde-99l).
    pub fn new(phy: P, conn: Connection) -> Self {
        Self { conn, phy }
    }

    /// Begin connecting (initiator).
    pub fn connect(&mut self, now: Duration) {
        self.conn.connect(now);
    }

    /// Enqueue a host message for reliable, in-order delivery.
    pub fn send(&mut self, data: Vec<u8>) {
        self.conn.send(data);
    }

    /// Begin a clean teardown.
    pub fn disconnect(&mut self, now: Duration) {
        self.conn.disconnect(now);
    }

    /// Apply a host/TNC command (#8). The complementary half of the contract is
    /// the [`HostEvent`] stream returned by [`Link::poll`].
    pub fn command(&mut self, cmd: HostCommand, now: Duration) {
        match cmd {
            HostCommand::Connect => self.conn.connect(now),
            HostCommand::Send(data) => self.conn.send(data),
            HostCommand::Disconnect => self.conn.disconnect(now),
        }
    }

    /// Current connection state.
    pub fn state(&self) -> ConnState {
        self.conn.state()
    }

    /// Pump one iteration at logical time `now`: ingest inbound frames, fire
    /// timers, flush this over's outbound frames, and return any host events.
    pub fn poll(&mut self, now: Duration) -> Vec<HostEvent> {
        // Ingest into a batch first so the channel measurement is fed BEFORE the SM
        // reacts (fresh worse-direction-wins). A failed decode (corruption the CRC
        // catches, or a collided fragment) is dropped, never delivered.
        let mut inbound = Vec::new();
        while let Some(rx) = self.phy.poll_rx() {
            if let Ok(frame) = LinkFrame::decode(rx.payload()) {
                inbound.push(frame);
            }
        }
        // Feed the measurement only on an actual decode (no stale re-apply).
        if !inbound.is_empty() {
            let q = self.phy.channel_quality();
            self.conn.observe_quality(
                q.aggregate_snr_db(),
                q.frame_error_rate(),
                q.recent_frames_total(),
            );
        }
        for frame in inbound {
            self.conn.handle_frame(frame, now);
        }
        // Advance timers (turn-recovery, retransmit, keepalive, death).
        self.conn.handle_timeout(now);
        // Flush the current over at the connection's CURRENT mode, so a mid-session
        // rung change reaches the wire (matches the Driver).
        while let Some(frame) = self.conn.poll_transmit(now) {
            if let Ok(bytes) = frame.encode() {
                let _ = self.phy.send_frame(&bytes, self.conn.current_hint());
            }
        }
        // Drain host events.
        let mut events = Vec::new();
        while let Some(e) = self.conn.poll_event() {
            events.push(e);
        }
        events
    }
}
