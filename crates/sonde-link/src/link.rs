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

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;

use crate::conn::{ConnState, Connection, HostEvent};
use crate::frame::LinkFrame;
use crate::host::HostCommand;

/// Drives a [`Connection`] over a `PhyTransport`.
pub struct Link<P: PhyTransport> {
    conn: Connection,
    phy: P,
    hint: ModeHint,
}

impl<P: PhyTransport> Link<P> {
    /// Wrap a connection and a PHY/transport with the mode hint to transmit under.
    pub fn new(phy: P, conn: Connection, hint: ModeHint) -> Self {
        Self { conn, phy, hint }
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
        // Ingest: decode each received frame; a failed decode (corruption the
        // CRC catches, or a collided fragment) is dropped, never delivered.
        while let Some(rx) = self.phy.poll_rx() {
            if let Ok(frame) = LinkFrame::decode(rx.payload()) {
                self.conn.handle_frame(frame, now);
            }
        }
        // Advance timers (turn-recovery, retransmit, keepalive, death).
        self.conn.handle_timeout(now);
        // Flush the current over to the wire.
        while let Some(frame) = self.conn.poll_transmit(now) {
            if let Ok(bytes) = frame.encode() {
                let _ = self.phy.send_frame(&bytes, self.hint);
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
