//! Minimal host / TNC command surface (#8).
//!
//! The host application drives the link with [`HostCommand`]s and observes it
//! through the [`HostEvent`](crate::HostEvent) stream the link already emits
//! (`Connected` / `DataReceived` / `Disconnected` / `PeerLost`). That pair —
//! commands in, events out — is the whole TNC contract: connect, send, receive
//! (as events), disconnect. The link owns all timing and retransmission; the
//! host only expresses intent.

/// A command from the host application to the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCommand {
    /// Open a connection to the peer (initiator).
    Connect,
    /// Send a host message for reliable, in-order delivery.
    Send(Vec<u8>),
    /// Tear the connection down cleanly.
    Disconnect,
}
