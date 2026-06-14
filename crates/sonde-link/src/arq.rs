//! Selective-repeat ARQ, half-duplex window-per-over (design §5).
//!
//! Three single-responsibility pieces:
//! - [`SendWindow`]: retransmission bookkeeping. Holds unacked DATA payloads,
//!   emits the up-to-`W` frames for the current over, and applies the peer's
//!   cumulative + selective (SACK) ack — sliding the base and clearing acked
//!   frames so the next over carries only the *gaps* (burst-friendly; no
//!   go-back-N waste).
//! - [`RecvBuffer`]: ordering. Dedups, buffers out-of-order DATA up to the
//!   window, delivers strictly in order, and computes the `ACK_THROUGH` +
//!   `SACK` bitmap to send back.
//! - [`Reassembler`]: message boundaries. Concatenates the in-order run of
//!   fragments and emits a whole host message when `END_OF_MSG` arrives.
//!
//! DATA sequence numbers start at 1 within a connection; `ack_through == 0`
//! means "nothing acknowledged yet". `u32` width ⇒ no wrap under long sessions.

use std::collections::BTreeMap;

/// Default selective-repeat window (frames per over). Floor/whole-message mode
/// degenerates this to 1 (design §5).
pub const DEFAULT_WINDOW: u32 = 8;

/// First DATA sequence number in a connection.
pub const FIRST_SEQ: u32 = 1;

/// Sender-side selective-repeat window.
#[derive(Debug)]
pub struct SendWindow {
    base: u32,
    next_seq: u32,
    window: u32,
    frames: BTreeMap<u32, Vec<u8>>,
}

impl SendWindow {
    /// New sender window with the given width. Window `1` is the floor
    /// whole-message strategy.
    pub fn new(window: u32) -> Self {
        assert!(window >= 1, "window must be >= 1");
        Self {
            base: FIRST_SEQ,
            next_seq: FIRST_SEQ,
            window,
            frames: BTreeMap::new(),
        }
    }

    /// Buffer a payload as the next DATA frame; returns its assigned seq.
    /// Buffering is unbounded (the host's bytes wait); the *window* bounds how
    /// many are transmitted per over.
    pub fn enqueue(&mut self, payload: Vec<u8>) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.frames.insert(seq, payload);
        seq
    }

    /// Clear all state back to a fresh session (keeps the configured window).
    /// Used when a connection closes so a same-object reconnect starts clean.
    pub fn reset(&mut self) {
        self.base = FIRST_SEQ;
        self.next_seq = FIRST_SEQ;
        self.frames.clear();
    }

    /// Change the window **in place**, preserving `base`/`next_seq` and all
    /// buffered-but-unacked frames. Used by mid-session mode adaptation: a mode
    /// change resizes the window without dropping the seq stream or outstanding
    /// data. `window == 1` is the floor whole-message strategy.
    pub fn reconfigure(&mut self, window: u32) {
        assert!(window >= 1, "window must be >= 1");
        self.window = window;
    }

    /// Whether any enqueued frame is still unacked.
    pub fn has_unacked(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Count of unacked frames still buffered.
    pub fn outstanding(&self) -> usize {
        self.frames.len()
    }

    /// The frames to transmit in the current over: buffered seqs within
    /// `[base, base + window)`, ascending. These are exactly the gaps plus any
    /// fresh frames that fit the window — the selective-repeat retransmit set.
    pub fn over_frames(&self) -> Vec<(u32, Vec<u8>)> {
        let limit = self.base.saturating_add(self.window);
        self.frames
            .range(self.base..limit)
            .map(|(&seq, payload)| (seq, payload.clone()))
            .collect()
    }

    /// Apply a peer ack: `ack_through` is the cumulative in-order high-water,
    /// `sack` is the selective bitmap (bit `i` ⇒ `ack_through + 1 + i` received
    /// out of order). Clears acked frames and slides the base past the
    /// cumulative high-water.
    pub fn on_ack(&mut self, ack_through: u32, sack: u32) {
        // Cumulative: clear everything at or below the high-water and slide base.
        self.frames.retain(|&seq, _| seq > ack_through);
        if self.base <= ack_through {
            self.base = ack_through + 1;
        }
        // Selective: bit i ⇒ seq (ack_through + 1 + i) received out of order.
        for i in 0..u32::BITS {
            if sack & (1 << i) != 0 {
                let seq = ack_through.saturating_add(1).saturating_add(i);
                self.frames.remove(&seq);
            }
        }
    }
}

/// One in-order delivered DATA frame handed up for reassembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    /// The fragment payload.
    pub payload: Vec<u8>,
    /// Whether this fragment ends a host message (`END_OF_MSG`).
    pub end_of_msg: bool,
}

/// Receiver-side ordering buffer.
#[derive(Debug)]
pub struct RecvBuffer {
    next_expected: u32,
    window: u32,
    buffered: BTreeMap<u32, Delivered>,
}

impl RecvBuffer {
    /// New receiver buffer with the given window width.
    pub fn new(window: u32) -> Self {
        assert!(window >= 1, "window must be >= 1");
        Self {
            next_expected: FIRST_SEQ,
            window,
            buffered: BTreeMap::new(),
        }
    }

    /// Accept a DATA frame. Returns the run of frames now deliverable strictly
    /// in order (empty if this frame is a duplicate, out-of-window, or fills no
    /// in-order gap). Never delivers a seq twice; never delivers out of order.
    pub fn accept(&mut self, seq: u32, payload: Vec<u8>, end_of_msg: bool) -> Vec<Delivered> {
        // Duplicate at or below the in-order high-water, or already buffered.
        if seq < self.next_expected || self.buffered.contains_key(&seq) {
            return Vec::new();
        }
        let frag = Delivered {
            payload,
            end_of_msg,
        };
        if seq != self.next_expected {
            // Out of order: buffer if within the receive window, else drop.
            if seq < self.next_expected.saturating_add(self.window) {
                self.buffered.insert(seq, frag);
            }
            return Vec::new();
        }
        // In order: deliver, then drain any consecutive buffered frames.
        let mut out = vec![frag];
        self.next_expected += 1;
        while let Some(next) = self.buffered.remove(&self.next_expected) {
            out.push(next);
            self.next_expected += 1;
        }
        out
    }

    /// Clear all state back to a fresh session (keeps the configured window).
    pub fn reset(&mut self) {
        self.next_expected = FIRST_SEQ;
        self.buffered.clear();
    }

    /// Change the window **in place**, preserving `next_expected` and all
    /// buffered out-of-order frames (mid-session mode adaptation).
    pub fn reconfigure(&mut self, window: u32) {
        assert!(window >= 1, "window must be >= 1");
        self.window = window;
    }

    /// Cumulative in-order high-water received (`0` ⇒ nothing yet).
    pub fn ack_through(&self) -> u32 {
        self.next_expected - 1
    }

    /// Selective-ack bitmap relative to `ack_through`: bit `i` set ⇒
    /// `ack_through + 1 + i` is buffered out of order.
    pub fn sack(&self) -> u32 {
        let mut bm = 0u32;
        for &seq in self.buffered.keys() {
            let i = seq - self.next_expected; // >= 1 (next_expected is the gap)
            if i < u32::BITS {
                bm |= 1 << i;
            }
        }
        bm
    }
}

/// Reassembles the in-order fragment stream into whole host messages.
#[derive(Debug, Default)]
pub struct Reassembler {
    buf: Vec<u8>,
}

impl Reassembler {
    /// New, empty reassembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one in-order delivered fragment. Returns `Some(message)` when the
    /// fragment ends a host message, else `None` (more fragments expected).
    pub fn push(&mut self, frag: Delivered) -> Option<Vec<u8>> {
        self.buf.extend_from_slice(&frag.payload);
        if frag.end_of_msg {
            Some(std::mem::take(&mut self.buf))
        } else {
            None
        }
    }

    /// Discard any partially-accumulated message (used on session reset so a
    /// half-received message never bleeds into a later session).
    pub fn reset(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- SendWindow ----

    #[test]
    fn enqueue_assigns_ascending_seqs_from_one() {
        let mut w = SendWindow::new(DEFAULT_WINDOW);
        assert_eq!(w.enqueue(b"a".to_vec()), 1);
        assert_eq!(w.enqueue(b"b".to_vec()), 2);
        assert_eq!(w.enqueue(b"c".to_vec()), 3);
        assert_eq!(w.outstanding(), 3);
        assert!(w.has_unacked());
    }

    #[test]
    fn over_frames_is_capped_at_window_and_starts_at_base() {
        let mut w = SendWindow::new(2);
        for b in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()] {
            w.enqueue(b);
        }
        let over = w.over_frames();
        assert_eq!(over.len(), 2, "window of 2 caps the over");
        assert_eq!(over[0].0, 1);
        assert_eq!(over[1].0, 2);
    }

    #[test]
    fn cumulative_ack_slides_base_and_reveals_next_batch() {
        let mut w = SendWindow::new(2);
        for b in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()] {
            w.enqueue(b);
        }
        w.on_ack(2, 0); // seqs 1,2 cumulatively acked
        assert_eq!(w.outstanding(), 2);
        let over = w.over_frames();
        assert_eq!(over[0].0, 3);
        assert_eq!(over[1].0, 4);
    }

    #[test]
    fn sack_retransmits_only_the_gap_not_the_selectively_acked() {
        let mut w = SendWindow::new(8);
        for b in [b"1".to_vec(), b"2".to_vec(), b"3".to_vec(), b"4".to_vec()] {
            w.enqueue(b);
        }
        // Peer got nothing cumulatively (ack_through 0) but received 2,3,4 OOO
        // (gap at seq 1). bit i ⇒ seq 1+i: seqs 2,3,4 ⇒ bits 1,2,3.
        w.on_ack(0, 0b1110);
        let over = w.over_frames();
        assert_eq!(over.len(), 1, "only the gap (seq 1) is retransmitted");
        assert_eq!(over[0].0, 1);
    }

    #[test]
    fn fully_acked_window_has_no_unacked() {
        let mut w = SendWindow::new(4);
        w.enqueue(b"a".to_vec());
        w.enqueue(b"b".to_vec());
        w.on_ack(2, 0);
        assert!(!w.has_unacked());
        assert!(w.over_frames().is_empty());
    }

    #[test]
    fn duplicate_or_stale_ack_is_idempotent() {
        let mut w = SendWindow::new(4);
        w.enqueue(b"a".to_vec());
        w.enqueue(b"b".to_vec());
        w.on_ack(2, 0);
        w.on_ack(1, 0); // stale, lower high-water
        w.on_ack(2, 0); // duplicate
        assert!(!w.has_unacked());
    }

    // ---- RecvBuffer ----

    #[test]
    fn in_order_frames_deliver_immediately() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        assert_eq!(r.accept(1, b"a".to_vec(), false).len(), 1);
        assert_eq!(r.accept(2, b"b".to_vec(), false).len(), 1);
        assert_eq!(r.ack_through(), 2);
        assert_eq!(r.sack(), 0);
    }

    #[test]
    fn out_of_order_frame_is_buffered_then_drained_on_gap_fill() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        assert!(
            r.accept(2, b"b".to_vec(), false).is_empty(),
            "2 buffered, gap at 1"
        );
        assert_eq!(r.ack_through(), 0);
        assert_eq!(r.sack(), 0b10, "seq 2 = bit 1");
        let drained = r.accept(1, b"a".to_vec(), false);
        assert_eq!(drained.len(), 2, "1 then 2 both deliver");
        assert_eq!(drained[0].payload, b"a");
        assert_eq!(drained[1].payload, b"b");
        assert_eq!(r.ack_through(), 2);
        assert_eq!(r.sack(), 0);
    }

    #[test]
    fn duplicate_below_high_water_is_dropped() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        r.accept(1, b"a".to_vec(), false);
        assert!(
            r.accept(1, b"a".to_vec(), false).is_empty(),
            "dup never re-delivered"
        );
    }

    #[test]
    fn duplicate_buffered_ooo_frame_is_dropped() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        r.accept(3, b"c".to_vec(), false);
        assert!(r.accept(3, b"c".to_vec(), false).is_empty());
        assert_eq!(r.sack(), 0b100, "only seq 3 = bit 2 buffered once");
    }

    #[test]
    fn sack_reflects_multiple_buffered_gaps() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        r.accept(2, b"b".to_vec(), false);
        r.accept(4, b"d".to_vec(), false);
        // next_expected = 1; seq2 ⇒ bit1, seq4 ⇒ bit3.
        assert_eq!(r.sack(), 0b1010);
    }

    #[test]
    fn end_of_msg_flag_is_preserved_through_ordering() {
        let mut r = RecvBuffer::new(DEFAULT_WINDOW);
        let d = r.accept(1, b"only".to_vec(), true);
        assert_eq!(d.len(), 1);
        assert!(d[0].end_of_msg);
    }

    // ---- Reassembler ----

    #[test]
    fn single_fragment_message_emits_on_end_of_msg() {
        let mut a = Reassembler::new();
        assert_eq!(
            a.push(Delivered {
                payload: b"hello".to_vec(),
                end_of_msg: true
            }),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn multi_fragment_message_concatenates_until_end_of_msg() {
        let mut a = Reassembler::new();
        assert_eq!(
            a.push(Delivered {
                payload: b"foo".to_vec(),
                end_of_msg: false
            }),
            None
        );
        assert_eq!(
            a.push(Delivered {
                payload: b"bar".to_vec(),
                end_of_msg: false
            }),
            None
        );
        assert_eq!(
            a.push(Delivered {
                payload: b"baz".to_vec(),
                end_of_msg: true
            }),
            Some(b"foobarbaz".to_vec())
        );
    }

    #[test]
    fn reassembler_resets_between_messages() {
        let mut a = Reassembler::new();
        a.push(Delivered {
            payload: b"msg1".to_vec(),
            end_of_msg: true,
        });
        assert_eq!(
            a.push(Delivered {
                payload: b"msg2".to_vec(),
                end_of_msg: true
            }),
            Some(b"msg2".to_vec()),
            "second message must not include the first"
        );
    }

    // ---- in-place reconfigure (mid-session mode change) ----

    #[test]
    fn send_window_reconfigure_preserves_in_flight_and_resizes() {
        let mut w = SendWindow::new(8);
        for b in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()] {
            w.enqueue(b);
        }
        assert_eq!(w.over_frames().len(), 4, "window 8 sends all 4");
        w.reconfigure(1); // drop to floor whole-message
        assert_eq!(w.outstanding(), 4, "no frames dropped by the resize");
        let over = w.over_frames();
        assert_eq!(over.len(), 1, "window 1 now caps the over");
        assert_eq!(over[0].0, 1, "still starts at the preserved base seq");
    }

    #[test]
    fn recv_buffer_reconfigure_preserves_high_water_and_buffered() {
        let mut r = RecvBuffer::new(8);
        r.accept(1, b"a".to_vec(), false);
        r.accept(3, b"c".to_vec(), false); // out-of-order, buffered
        assert_eq!(r.ack_through(), 1);
        assert_ne!(r.sack(), 0);
        r.reconfigure(1);
        assert_eq!(r.ack_through(), 1, "high-water preserved across resize");
        assert_ne!(r.sack(), 0, "buffered out-of-order frame preserved");
    }
}
