//! The on-air link frame: framing, turn token, sequence/ACK fields, CRC.
//!
//! Wire layout (big-endian), one frame = one PHY `send_frame`:
//!
//! ```text
//! off field        size  notes
//! 0   MAGIC 53 4C   2
//! 2   VER           1    = 2
//! 3   TYPE          1    DATA/ACK/NAK/CONN/CONN_ACK/DISC/DISC_ACK/KEEPALIVE/ID
//! 4   FLAGS         1    bit0 END_OF_OVER (this frame ends the sender's over → floor passes)
//!                        bit1 END_OF_MSG  (last fragment of a host message)
//! 5   CONN_ID       2    session id — THE demux/routing key (rejects half-open / cross-session)
//! 7   SEQ           4    DATA frame seq, or control context seq
//! 11  ACK_THROUGH   4    cumulative in-order high-water acknowledged
//! 15  SACK          4    bitmap: bit i ⇒ (ACK_THROUGH+1+i) received out-of-order
//! 19  MODE          1    link mode/rung id this frame was sent under (link adaptation)
//! 20  LEN           2    payload-region length
//! 22  PAYLOAD       N
//! 22+N CRC32        4    IEEE, over [0 .. 22+N)
//! ```
//!
//! **Callsigns are NOT in every frame.** Per Part-97 §97.119 a station identifies
//! at the *start* and *end* of a communication and *at least every 10 minutes* —
//! never per frame (the per-frame callsign was ~half the header and, on a
//! deep-floor over, larger than the payload). Callsigns therefore ride only on the
//! **ID-bearing** frame types (`CONN`, `CONN_ACK`, `DISC`, `DISC_ACK`, `ID`), where
//! the 22-byte payload region carries a [`StationId`] (`SRC[10]` + `DST[10]`,
//! NUL-padded). The data plane (`DATA`/`ACK`/`NAK`/`KEEPALIVE`) is addressed
//! purely by `CONN_ID`. See `docs/superpowers/specs/2026-06-15-frame-conn-id-addressing-design.md`.
//!
//! Parsing is **exact-length and CRC-first**: a frame is rejected unless its
//! buffer length is exactly `LINK_OVERHEAD + LEN` *and* the CRC verifies, before
//! any field is trusted. A corrupted `LEN` therefore cannot walk the CRC offset.
//!
//! `END_OF_OVER` is the explicit turn token: the `PhyTransport` seam exposes no
//! carrier-sense, so the floor is passed in-band rather than inferred from
//! silence (see the crate design doc §3.5).

use crc::{Crc, CRC_32_ISO_HDLC};
use thiserror::Error;

/// Frame magic ("SL").
pub const MAGIC: [u8; 2] = [0x53, 0x4C];
/// Link protocol version this build speaks. Bumped to 2 with the callsign-removal
/// wire change: a v1 frame now fails [`FrameError::BadVersion`] rather than
/// silently mis-parsing under the old (callsign-bearing) layout.
pub const VERSION: u8 = 2;
/// Fixed bytes before the payload region (MAGIC..=LEN).
pub const HEADER_LEN: usize = 22;
/// CRC trailer length.
pub const CRC_LEN: usize = 4;
/// Total fixed per-frame overhead (header + CRC).
pub const LINK_OVERHEAD: usize = HEADER_LEN + CRC_LEN;
/// Maximum payload bytes carriable in one frame (the PHY caps total frame bytes
/// at `u16::MAX`; the header+CRC eat `LINK_OVERHEAD`).
pub const LINK_MTU: usize = u16::MAX as usize - LINK_OVERHEAD;

/// One wire callsign field width (NUL-padded).
const CALLSIGN_WIRE_LEN: usize = 10;
/// The station-ID block is exactly two callsign fields (`SRC` + `DST`).
pub const STATION_ID_LEN: usize = 2 * CALLSIGN_WIRE_LEN;

/// FLAGS bit 0: this frame is the last of the sender's over; receiving it passes
/// the floor (turn token). See design §3.5.
pub const FLAG_END_OF_OVER: u8 = 0x01;
/// FLAGS bit 1: this frame is the last fragment of a host message; the receiver
/// reassembles the in-order run of DATA frames ending here into one message.
pub const FLAG_END_OF_MSG: u8 = 0x02;

/// FLAGS bits 2–4: `rx_rung` — the sender's *recommended* ladder rung for the
/// peer's transmissions (link mode-adaptation feedback). The receiver of an over
/// is the only party that observed how well it decoded, so it advertises the rung
/// the peer should not exceed. 3 bits hold the 5-rung ladder (0..=BASE). Packed
/// into spare FLAGS bits (zero added header bytes — airtime). Bits 5–7 reserved
/// (must be 0). See `docs/superpowers/specs/2026-06-15-downshift-control-loop-design.md`.
const FLAG_RX_RUNG_SHIFT: u8 = 2;
const FLAG_RX_RUNG_MASK: u8 = 0b0001_1100;

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// Link frame type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// Payload-bearing data frame.
    Data = 1,
    /// Cumulative + selective acknowledgement (carries `ACK_THROUGH` + `SACK`).
    Ack = 2,
    /// Explicit negative ack (optional; SACK is the primary loss signal).
    Nak = 3,
    /// Connection request. ID-bearing (start).
    Conn = 4,
    /// Connection acceptance. ID-bearing (start).
    ConnAck = 5,
    /// Disconnect request. ID-bearing (end).
    Disc = 6,
    /// Disconnect acknowledgement. ID-bearing (end).
    DiscAck = 7,
    /// Idle keepalive (rides through dead air without tearing down).
    Keepalive = 8,
    /// Periodic station identification (Part-97 §97.119, ≤10 min). ID-bearing.
    Id = 9,
}

impl FrameType {
    fn from_u8(b: u8) -> Result<Self, FrameError> {
        Ok(match b {
            1 => Self::Data,
            2 => Self::Ack,
            3 => Self::Nak,
            4 => Self::Conn,
            5 => Self::ConnAck,
            6 => Self::Disc,
            7 => Self::DiscAck,
            8 => Self::Keepalive,
            9 => Self::Id,
            other => return Err(FrameError::UnknownType(other)),
        })
    }

    /// Whether this frame type carries a [`StationId`] (callsigns) in its payload
    /// region. ID-bearing frames are the only ones that identify the station, per
    /// Part-97 §97.119: `CONN`/`CONN_ACK` (start), `DISC`/`DISC_ACK` (end), and
    /// the periodic `ID`. All others are addressed by `CONN_ID` alone.
    pub fn is_id_bearing(self) -> bool {
        matches!(
            self,
            Self::Conn | Self::ConnAck | Self::Disc | Self::DiscAck | Self::Id
        )
    }
}

/// A validated amateur station callsign (Part-97 station ID). 1–10 characters of
/// `[A-Z0-9/]`; ASCII letters are upper-cased on construction. Stored NUL-padded
/// to the 10-byte wire field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Callsign(String);

impl Callsign {
    /// Validate and construct. ASCII letters are normalized to upper case.
    pub fn new(s: &str) -> Result<Self, FrameError> {
        let up = s.to_ascii_uppercase();
        if up.is_empty() || up.len() > CALLSIGN_WIRE_LEN {
            return Err(FrameError::BadCallsign(s.to_string()));
        }
        if !up
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
        {
            return Err(FrameError::BadCallsign(s.to_string()));
        }
        Ok(Self(up))
    }

    /// The callsign as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn to_wire(&self) -> [u8; CALLSIGN_WIRE_LEN] {
        let mut w = [0u8; CALLSIGN_WIRE_LEN];
        w[..self.0.len()].copy_from_slice(self.0.as_bytes());
        w
    }

    fn from_wire(w: &[u8]) -> Result<Self, FrameError> {
        let end = w.iter().position(|&b| b == 0).unwrap_or(w.len());
        // Canonical padding: everything after the first NUL must also be NUL, so a
        // single wire encoding maps to a single callsign (no smuggled bytes).
        if w[end..].iter().any(|&b| b != 0) {
            return Err(FrameError::BadCallsign("<non-canonical padding>".into()));
        }
        let s = std::str::from_utf8(&w[..end])
            .map_err(|_| FrameError::BadCallsign("<non-utf8>".into()))?;
        Self::new(s)
    }
}

/// The two station callsigns carried on an ID-bearing frame (`SRC` then `DST`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationId {
    /// Transmitting station (Part-97 ID of the sender).
    pub src: Callsign,
    /// Destination station this frame is addressed to.
    pub dst: Callsign,
}

impl StationId {
    /// Construct a station-ID block from source + destination callsigns.
    pub fn new(src: Callsign, dst: Callsign) -> Self {
        Self { src, dst }
    }
}

/// A parsed/serializable link frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFrame {
    /// Frame type.
    pub frame_type: FrameType,
    /// Control flags (see [`FLAG_END_OF_OVER`]).
    pub flags: u8,
    /// Session id (0 before a connection is established) — the routing key.
    pub conn_id: u16,
    /// Data sequence number, or control context sequence.
    pub seq: u32,
    /// Cumulative in-order high-water acknowledged (0 if not carrying an ack).
    pub ack_through: u32,
    /// Selective-ack bitmap relative to `ack_through`.
    pub sack: u32,
    /// Link mode / ladder-rung id this frame was transmitted under (link
    /// adaptation). `0` = the default/fastest rung; the receiver reads it to
    /// follow a downshift and to detect mode divergence.
    pub mode: u8,
    /// Station callsigns — `Some` **iff** `frame_type.is_id_bearing()`. Occupies
    /// the payload region on ID-bearing frames; `None` on the data plane.
    pub id: Option<StationId>,
    /// Payload bytes (host data; empty for control and ID-bearing frames).
    pub payload: Vec<u8>,
}

impl LinkFrame {
    /// Build a DATA frame (flags clear; set the turn token with [`Self::end_of_over`]).
    pub fn data(conn_id: u16, seq: u32, payload: Vec<u8>) -> Self {
        Self {
            frame_type: FrameType::Data,
            flags: 0,
            conn_id,
            seq,
            ack_through: 0,
            sack: 0,
            mode: 0,
            id: None,
            payload,
        }
    }

    /// Build an ACK frame carrying cumulative + selective acknowledgement.
    pub fn ack(conn_id: u16, ack_through: u32, sack: u32) -> Self {
        Self {
            frame_type: FrameType::Ack,
            flags: 0,
            conn_id,
            seq: 0,
            ack_through,
            sack,
            mode: 0,
            id: None,
            payload: Vec::new(),
        }
    }

    /// Build a bare (non-ID-bearing) control frame — `KEEPALIVE` or `NAK`.
    /// ID-bearing control frames are built with [`Self::id_control`].
    pub fn control(frame_type: FrameType, conn_id: u16, seq: u32) -> Self {
        Self {
            frame_type,
            flags: 0,
            conn_id,
            seq,
            ack_through: 0,
            sack: 0,
            mode: 0,
            id: None,
            payload: Vec::new(),
        }
    }

    /// Build an ID-bearing control frame (`CONN`/`CONN_ACK`/`DISC`/`DISC_ACK`)
    /// carrying the station-ID block (Part-97 start/end identification).
    pub fn id_control(frame_type: FrameType, station: StationId, conn_id: u16, seq: u32) -> Self {
        Self {
            frame_type,
            flags: 0,
            conn_id,
            seq,
            ack_through: 0,
            sack: 0,
            mode: 0,
            id: Some(station),
            payload: Vec::new(),
        }
    }

    /// Build a periodic `ID` frame (Part-97 §97.119, ≤10 min identification).
    pub fn id_frame(station: StationId, conn_id: u16) -> Self {
        Self::id_control(FrameType::Id, station, conn_id, 0)
    }

    /// Mark this as the last frame of the sender's over (sets the turn token).
    pub fn end_of_over(mut self) -> Self {
        self.flags |= FLAG_END_OF_OVER;
        self
    }

    /// Whether this frame ends the sender's over (passes the floor).
    pub fn is_end_of_over(&self) -> bool {
        self.flags & FLAG_END_OF_OVER != 0
    }

    /// Mark this as the last fragment of a host message (sets `END_OF_MSG`).
    pub fn end_of_msg(mut self) -> Self {
        self.flags |= FLAG_END_OF_MSG;
        self
    }

    /// Whether this frame is the last fragment of a host message.
    pub fn is_end_of_msg(&self) -> bool {
        self.flags & FLAG_END_OF_MSG != 0
    }

    /// Stamp the link mode / ladder-rung id this frame is sent under.
    pub fn with_mode(mut self, mode: u8) -> Self {
        self.mode = mode;
        self
    }

    /// Stamp the receiver-feedback `rx_rung` (the rung this station recommends the
    /// peer use) into FLAGS bits 2–4, preserving the END tokens (bits 0–1) and the
    /// reserved bits (5–7). Values are masked to 3 bits.
    pub fn with_rx_rung(mut self, rung: u8) -> Self {
        self.flags =
            (self.flags & !FLAG_RX_RUNG_MASK) | ((rung << FLAG_RX_RUNG_SHIFT) & FLAG_RX_RUNG_MASK);
        self
    }

    /// The receiver-feedback `rx_rung` carried in FLAGS bits 2–4 (0..=7; the link
    /// layer clamps it to the ladder).
    pub fn rx_rung(&self) -> u8 {
        (self.flags & FLAG_RX_RUNG_MASK) >> FLAG_RX_RUNG_SHIFT
    }

    /// Serialize to wire bytes (header + payload region + CRC32). Enforces the
    /// invariant `frame_type.is_id_bearing() ⟺ id.is_some()` and that ID-bearing
    /// frames carry no host payload (the region is exactly the station-ID block).
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let id_bearing = self.frame_type.is_id_bearing();
        if id_bearing != self.id.is_some() {
            return Err(FrameError::IdPresenceMismatch {
                frame_type: self.frame_type as u8,
                has_id: self.id.is_some(),
            });
        }
        // The payload region is either the station-ID block (ID-bearing) or the
        // host payload (data plane) — never both.
        let region: Vec<u8> = if let Some(id) = &self.id {
            if !self.payload.is_empty() {
                return Err(FrameError::IdFrameHasPayload);
            }
            let mut r = Vec::with_capacity(STATION_ID_LEN);
            r.extend_from_slice(&id.src.to_wire());
            r.extend_from_slice(&id.dst.to_wire());
            r
        } else {
            self.payload.clone()
        };
        if region.len() > LINK_MTU {
            return Err(FrameError::PayloadTooLarge {
                len: region.len(),
                max: LINK_MTU,
            });
        }
        let mut b = Vec::with_capacity(LINK_OVERHEAD + region.len());
        b.extend_from_slice(&MAGIC);
        b.push(VERSION);
        b.push(self.frame_type as u8);
        b.push(self.flags);
        b.extend_from_slice(&self.conn_id.to_be_bytes());
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.ack_through.to_be_bytes());
        b.extend_from_slice(&self.sack.to_be_bytes());
        b.push(self.mode);
        b.extend_from_slice(&(region.len() as u16).to_be_bytes());
        b.extend_from_slice(&region);
        let crc = CRC32.checksum(&b);
        b.extend_from_slice(&crc.to_be_bytes());
        Ok(b)
    }

    /// Parse wire bytes. Exact-length and CRC-first: rejects unless the buffer is
    /// exactly `LINK_OVERHEAD + LEN` long and the CRC verifies. ID-bearing types
    /// (decided by the CRC-verified TYPE byte) require `LEN == STATION_ID_LEN` and
    /// parse the region into a [`StationId`]; all others treat it as host payload.
    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        if buf.len() < LINK_OVERHEAD {
            return Err(FrameError::TooShort { got: buf.len() });
        }
        if buf[0..2] != MAGIC {
            return Err(FrameError::BadMagic);
        }
        if buf[2] != VERSION {
            return Err(FrameError::BadVersion { got: buf[2] });
        }
        let frame_type = FrameType::from_u8(buf[3])?;
        let flags = buf[4];
        let len = u16::from_be_bytes([buf[20], buf[21]]) as usize;
        let expected = LINK_OVERHEAD + len;
        if buf.len() != expected {
            return Err(FrameError::LengthMismatch {
                expected,
                got: buf.len(),
            });
        }
        let crc_calc = CRC32.checksum(&buf[..HEADER_LEN + len]);
        let c = HEADER_LEN + len;
        let crc_read = u32::from_be_bytes([buf[c], buf[c + 1], buf[c + 2], buf[c + 3]]);
        if crc_calc != crc_read {
            return Err(FrameError::BadCrc);
        }
        let conn_id = u16::from_be_bytes([buf[5], buf[6]]);
        let seq = u32::from_be_bytes([buf[7], buf[8], buf[9], buf[10]]);
        let ack_through = u32::from_be_bytes([buf[11], buf[12], buf[13], buf[14]]);
        let sack = u32::from_be_bytes([buf[15], buf[16], buf[17], buf[18]]);
        let mode = buf[19];
        let region = &buf[HEADER_LEN..HEADER_LEN + len];
        let (id, payload) = if frame_type.is_id_bearing() {
            if len != STATION_ID_LEN {
                return Err(FrameError::BadStationIdLen { got: len });
            }
            let src = Callsign::from_wire(&region[0..CALLSIGN_WIRE_LEN])?;
            let dst = Callsign::from_wire(&region[CALLSIGN_WIRE_LEN..STATION_ID_LEN])?;
            (Some(StationId { src, dst }), Vec::new())
        } else {
            (None, region.to_vec())
        };
        Ok(Self {
            frame_type,
            flags,
            conn_id,
            seq,
            ack_through,
            sack,
            mode,
            id,
            payload,
        })
    }
}

/// Errors from frame construction and parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    /// Buffer shorter than the fixed overhead.
    #[error("frame too short: {got} bytes < overhead")]
    TooShort {
        /// Bytes received.
        got: usize,
    },
    /// Magic bytes did not match.
    #[error("bad magic")]
    BadMagic,
    /// Unsupported protocol version.
    #[error("unsupported link version: {got}")]
    BadVersion {
        /// Version byte seen.
        got: u8,
    },
    /// Unknown frame type byte.
    #[error("unknown frame type: {0}")]
    UnknownType(u8),
    /// Declared length did not match the buffer (corruption or truncation).
    #[error("length mismatch: expected {expected} bytes, got {got}")]
    LengthMismatch {
        /// `LINK_OVERHEAD + declared LEN`.
        expected: usize,
        /// Actual buffer length.
        got: usize,
    },
    /// CRC32 verification failed.
    #[error("crc mismatch")]
    BadCrc,
    /// Invalid callsign.
    #[error("invalid callsign: {0:?}")]
    BadCallsign(String),
    /// An ID-bearing frame's region was not exactly the station-ID block.
    #[error("bad station-id length: {got} (expected {STATION_ID_LEN})")]
    BadStationIdLen {
        /// The declared region length seen.
        got: usize,
    },
    /// `id` presence did not match `frame_type.is_id_bearing()` on encode.
    #[error("id presence mismatch: frame_type={frame_type}, has_id={has_id}")]
    IdPresenceMismatch {
        /// The frame type byte.
        frame_type: u8,
        /// Whether an `id` was present.
        has_id: bool,
    },
    /// An ID-bearing frame carried host payload (the region must be ID-only).
    #[error("id-bearing frame must not carry host payload")]
    IdFrameHasPayload,
    /// Payload exceeds the per-frame link MTU.
    #[error("payload too large: {len} > {max}")]
    PayloadTooLarge {
        /// Attempted payload length.
        len: usize,
        /// Per-frame MTU.
        max: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(s: &str) -> Callsign {
        Callsign::new(s).unwrap()
    }

    fn station() -> StationId {
        StationId::new(call("K1ABC"), call("W2XYZ"))
    }

    #[test]
    fn overhead_dropped_to_26_bytes() {
        // The callsign-removal win: 20 bytes (SRC+DST) gone from every frame.
        assert_eq!(HEADER_LEN, 22);
        assert_eq!(LINK_OVERHEAD, 26);
    }

    #[test]
    fn data_frame_round_trips_with_no_callsigns() {
        let f = LinkFrame::data(0x1234, 42, b"hello over".to_vec());
        let bytes = f.encode().unwrap();
        assert_eq!(bytes.len(), LINK_OVERHEAD + 10);
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(f, g);
        assert_eq!(g.id, None, "data plane carries no station id");
        assert_eq!(g.conn_id, 0x1234);
        assert_eq!(g.payload, b"hello over");
        assert!(!g.is_end_of_over());
    }

    #[test]
    fn end_of_over_turn_token_round_trips() {
        let f = LinkFrame::data(1, 7, b"x".to_vec()).end_of_over();
        assert!(f.is_end_of_over());
        let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
        assert!(g.is_end_of_over());
    }

    #[test]
    fn end_of_msg_is_independent_of_end_of_over() {
        let mid = LinkFrame::data(1, 3, b"frag".to_vec()).end_of_msg();
        assert!(mid.is_end_of_msg());
        assert!(!mid.is_end_of_over());
        let g = LinkFrame::decode(&mid.encode().unwrap()).unwrap();
        assert!(g.is_end_of_msg());
        assert!(!g.is_end_of_over());

        let both = LinkFrame::data(1, 4, b"last".to_vec())
            .end_of_msg()
            .end_of_over();
        assert!(both.is_end_of_msg());
        assert!(both.is_end_of_over());
    }

    #[test]
    fn ack_frame_round_trips_with_sack_and_zero_len() {
        let f = LinkFrame::ack(0x1234, 100, 0b1011);
        let bytes = f.encode().unwrap();
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(g.frame_type, FrameType::Ack);
        assert_eq!(g.ack_through, 100);
        assert_eq!(g.sack, 0b1011);
        assert!(g.payload.is_empty());
        assert_eq!(g.id, None);
        assert_eq!(u16::from_be_bytes([bytes[20], bytes[21]]), 0); // LEN at offset 20
    }

    #[test]
    fn mode_id_round_trips_at_offset_19() {
        let f = LinkFrame::data(1, 5, b"x".to_vec()).with_mode(3);
        let bytes = f.encode().unwrap();
        assert_eq!(bytes[19], 3, "MODE at offset 19");
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(g.mode, 3);
        // Default mode is 0 when not stamped (on an ID-bearing CONN here).
        let h = LinkFrame::id_control(FrameType::Conn, station(), 1, 1);
        assert_eq!(LinkFrame::decode(&h.encode().unwrap()).unwrap().mode, 0);
    }

    #[test]
    fn conn_id_round_trips_at_offset_5() {
        let f = LinkFrame::data(0xBEEF, 1, b"x".to_vec());
        let bytes = f.encode().unwrap();
        assert_eq!(u16::from_be_bytes([bytes[5], bytes[6]]), 0xBEEF);
    }

    #[test]
    fn id_bearing_control_frames_round_trip_the_station_id() {
        for t in [
            FrameType::Conn,
            FrameType::ConnAck,
            FrameType::Disc,
            FrameType::DiscAck,
        ] {
            let f = LinkFrame::id_control(t, station(), 7, 9);
            let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
            assert_eq!(g.frame_type, t);
            assert_eq!(g.conn_id, 7);
            assert_eq!(g.id, Some(station()));
            assert_eq!(g.id.unwrap().src.as_str(), "K1ABC");
        }
    }

    #[test]
    fn periodic_id_frame_round_trips() {
        let f = LinkFrame::id_frame(station(), 0x1234).with_mode(2);
        let bytes = f.encode().unwrap();
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(g.frame_type, FrameType::Id);
        assert_eq!(g.conn_id, 0x1234);
        assert_eq!(g.mode, 2);
        assert_eq!(g.id, Some(station()));
    }

    #[test]
    fn non_id_bearing_control_frames_carry_no_callsigns() {
        for t in [FrameType::Keepalive, FrameType::Nak] {
            let f = LinkFrame::control(t, 7, 9);
            let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
            assert_eq!(g.frame_type, t);
            assert_eq!(g.id, None);
            assert!(g.payload.is_empty());
        }
    }

    #[test]
    fn encode_rejects_id_bearing_type_without_a_station_id() {
        // A CONN built without callsigns must not silently go out callsign-less.
        let bad = LinkFrame::control(FrameType::Conn, 1, 1);
        assert_eq!(
            bad.encode(),
            Err(FrameError::IdPresenceMismatch {
                frame_type: FrameType::Conn as u8,
                has_id: false,
            })
        );
    }

    #[test]
    fn decode_rejects_id_bearing_frame_with_wrong_region_length() {
        // Forge a CONN whose region is not the 20-byte station-id block.
        let mut f = LinkFrame::data(1, 1, vec![0u8; 5]);
        f.frame_type = FrameType::Conn; // bypass the encode invariant via raw field
        f.id = None;
        // Encode would reject (presence mismatch); build the wire bytes by hand
        // off a DATA frame, then flip the TYPE byte and re-CRC.
        let data = LinkFrame::data(1, 1, vec![0u8; 5]);
        let mut bytes = data.encode().unwrap();
        bytes[3] = FrameType::Conn as u8; // now an ID-bearing type with LEN=5
        let crc = CRC32.checksum(&bytes[..HEADER_LEN + 5]);
        bytes[HEADER_LEN + 5..].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            LinkFrame::decode(&bytes),
            Err(FrameError::BadStationIdLen { got: 5 })
        );
    }

    #[test]
    fn crc_rejects_a_flipped_payload_byte() {
        let f = LinkFrame::data(1, 1, b"payload".to_vec());
        let mut bytes = f.encode().unwrap();
        bytes[HEADER_LEN] ^= 0x01;
        assert_eq!(LinkFrame::decode(&bytes), Err(FrameError::BadCrc));
    }

    #[test]
    fn crc_rejects_a_flipped_header_byte() {
        let f = LinkFrame::data(1, 1, b"x".to_vec());
        let mut bytes = f.encode().unwrap();
        bytes[7] ^= 0xFF; // corrupt SEQ (now at offset 7)
        assert_eq!(LinkFrame::decode(&bytes), Err(FrameError::BadCrc));
    }

    #[test]
    fn truncated_buffer_is_too_short() {
        assert!(matches!(
            LinkFrame::decode(&[0u8; 10]),
            Err(FrameError::TooShort { .. })
        ));
    }

    #[test]
    fn declared_length_longer_than_buffer_is_length_mismatch() {
        let f = LinkFrame::data(1, 1, b"abc".to_vec());
        let mut bytes = f.encode().unwrap();
        let fake = (1000u16).to_be_bytes();
        bytes[20] = fake[0];
        bytes[21] = fake[1];
        assert!(matches!(
            LinkFrame::decode(&bytes),
            Err(FrameError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn bad_magic_rejected() {
        let f = LinkFrame::id_control(FrameType::Conn, station(), 0, 0);
        let mut bytes = f.encode().unwrap();
        bytes[0] = 0x00;
        assert_eq!(LinkFrame::decode(&bytes), Err(FrameError::BadMagic));
    }

    #[test]
    fn old_version_byte_is_rejected_not_misparsed() {
        let f = LinkFrame::data(1, 1, b"x".to_vec());
        let mut bytes = f.encode().unwrap();
        bytes[2] = 1; // a v1 frame
                      // CRC will also fail, but version is checked first.
        assert_eq!(
            LinkFrame::decode(&bytes),
            Err(FrameError::BadVersion { got: 1 })
        );
    }

    #[test]
    fn callsign_validation() {
        assert!(Callsign::new("K1ABC").is_ok());
        assert!(Callsign::new("VE3AB/P").is_ok());
        assert_eq!(Callsign::new("k1abc").unwrap().as_str(), "K1ABC");
        assert!(Callsign::new("").is_err());
        assert!(Callsign::new("TOOLONGCALL1").is_err());
        assert!(Callsign::new("AB CD").is_err());
        assert!(Callsign::new("AB-CD").is_err());
    }

    #[test]
    fn non_canonical_callsign_padding_is_rejected() {
        let f = LinkFrame::id_control(FrameType::Conn, station(), 1, 1);
        let mut bytes = f.encode().unwrap();
        // SRC field is region[0..10] at HEADER_LEN..HEADER_LEN+10; "K1ABC" is 5
        // chars, so byte HEADER_LEN+6 is padding. Smuggle a byte after the NUL.
        bytes[HEADER_LEN + 6] = b'X';
        let crc = CRC32.checksum(&bytes[..HEADER_LEN + STATION_ID_LEN]);
        bytes[HEADER_LEN + STATION_ID_LEN..].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(
            LinkFrame::decode(&bytes),
            Err(FrameError::BadCallsign(_))
        ));
    }

    #[test]
    fn mtu_is_enforced_on_encode() {
        let f = LinkFrame::data(1, 1, vec![0u8; LINK_MTU + 1]);
        assert!(matches!(
            f.encode(),
            Err(FrameError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn rx_rung_packs_into_flags_without_disturbing_end_tokens() {
        // rx_rung coexists with both END flags and survives a wire round-trip.
        let f = LinkFrame::data(1, 1, b"x".to_vec())
            .end_of_over()
            .end_of_msg()
            .with_rx_rung(4);
        assert_eq!(f.rx_rung(), 4);
        assert!(f.is_end_of_over());
        assert!(f.is_end_of_msg());
        let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
        assert_eq!(g.rx_rung(), 4);
        assert!(g.is_end_of_over());
        assert!(g.is_end_of_msg());
        // Default is 0 when not stamped.
        assert_eq!(LinkFrame::data(1, 1, vec![]).rx_rung(), 0);
        // Restamping replaces only the rx_rung bits, leaving the END tokens.
        let h = f.with_rx_rung(1);
        assert_eq!(h.rx_rung(), 1);
        assert!(h.is_end_of_over());
        assert!(h.is_end_of_msg());
    }

    #[test]
    fn is_id_bearing_classification() {
        for t in [
            FrameType::Conn,
            FrameType::ConnAck,
            FrameType::Disc,
            FrameType::DiscAck,
            FrameType::Id,
        ] {
            assert!(t.is_id_bearing(), "{t:?} should be id-bearing");
        }
        for t in [
            FrameType::Data,
            FrameType::Ack,
            FrameType::Nak,
            FrameType::Keepalive,
        ] {
            assert!(!t.is_id_bearing(), "{t:?} should not be id-bearing");
        }
    }
}
