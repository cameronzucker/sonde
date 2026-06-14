//! The on-air link frame: framing, station ID, turn token, sequence/ACK fields, CRC.
//!
//! Wire layout (big-endian), one frame = one PHY `send_frame`:
//!
//! ```text
//! off field        size  notes
//! 0   MAGIC 53 4C   2
//! 2   VER           1    = 1
//! 3   TYPE          1    DATA/ACK/NAK/CONN/CONN_ACK/DISC/DISC_ACK/KEEPALIVE
//! 4   FLAGS         1    bit0 END_OF_OVER (this frame ends the sender's over → floor passes)
//! 5   SRC callsign  10   Part-97 station ID (validated), in EVERY frame
//! 15  DST callsign  10
//! 25  CONN_ID       2    session id (rejects half-open / cross-session)
//! 27  SEQ           4    DATA frame seq, or control context seq
//! 31  ACK_THROUGH   4    cumulative in-order high-water acknowledged
//! 35  SACK          4    bitmap: bit i ⇒ (ACK_THROUGH+1+i) received out-of-order
//! 39  LEN           2    payload length
//! 41  PAYLOAD       N
//! 41+N CRC32        4    IEEE, over [0 .. 41+N)
//! ```
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
/// Link protocol version this build speaks.
pub const VERSION: u8 = 1;
/// Fixed bytes before the payload (MAGIC..=LEN).
pub const HEADER_LEN: usize = 41;
/// CRC trailer length.
pub const CRC_LEN: usize = 4;
/// Total fixed per-frame overhead (header + CRC).
pub const LINK_OVERHEAD: usize = HEADER_LEN + CRC_LEN;
/// Maximum payload bytes carriable in one frame (the PHY caps total frame bytes
/// at `u16::MAX`; the header+CRC eat `LINK_OVERHEAD`).
pub const LINK_MTU: usize = u16::MAX as usize - LINK_OVERHEAD;

/// FLAGS bit 0: this frame is the last of the sender's over; receiving it passes
/// the floor (turn token). See design §3.5.
pub const FLAG_END_OF_OVER: u8 = 0x01;
/// FLAGS bit 1: this frame is the last fragment of a host message; the receiver
/// reassembles the in-order run of DATA frames ending here into one message.
pub const FLAG_END_OF_MSG: u8 = 0x02;

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
    /// Connection request.
    Conn = 4,
    /// Connection acceptance.
    ConnAck = 5,
    /// Disconnect request.
    Disc = 6,
    /// Disconnect acknowledgement.
    DiscAck = 7,
    /// Idle keepalive (rides through dead air without tearing down).
    Keepalive = 8,
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
            other => return Err(FrameError::UnknownType(other)),
        })
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
        if up.is_empty() || up.len() > 10 {
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

    fn to_wire(&self) -> [u8; 10] {
        let mut w = [0u8; 10];
        w[..self.0.len()].copy_from_slice(self.0.as_bytes());
        w
    }

    fn from_wire(w: &[u8]) -> Result<Self, FrameError> {
        let end = w.iter().position(|&b| b == 0).unwrap_or(w.len());
        let s = std::str::from_utf8(&w[..end])
            .map_err(|_| FrameError::BadCallsign("<non-utf8>".into()))?;
        Self::new(s)
    }
}

/// A parsed/serializable link frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFrame {
    /// Frame type.
    pub frame_type: FrameType,
    /// Control flags (see [`FLAG_END_OF_OVER`]).
    pub flags: u8,
    /// Source station (Part-97 ID).
    pub src: Callsign,
    /// Destination station.
    pub dst: Callsign,
    /// Session id (0 before a connection is established).
    pub conn_id: u16,
    /// Data sequence number, or control context sequence.
    pub seq: u32,
    /// Cumulative in-order high-water acknowledged (0 if not carrying an ack).
    pub ack_through: u32,
    /// Selective-ack bitmap relative to `ack_through`.
    pub sack: u32,
    /// Payload bytes (empty for pure control frames).
    pub payload: Vec<u8>,
}

impl LinkFrame {
    /// Build a DATA frame (flags clear; set the turn token with [`Self::end_of_over`]).
    pub fn data(src: Callsign, dst: Callsign, conn_id: u16, seq: u32, payload: Vec<u8>) -> Self {
        Self {
            frame_type: FrameType::Data,
            flags: 0,
            src,
            dst,
            conn_id,
            seq,
            ack_through: 0,
            sack: 0,
            payload,
        }
    }

    /// Build an ACK frame carrying cumulative + selective acknowledgement.
    pub fn ack(src: Callsign, dst: Callsign, conn_id: u16, ack_through: u32, sack: u32) -> Self {
        Self {
            frame_type: FrameType::Ack,
            flags: 0,
            src,
            dst,
            conn_id,
            seq: 0,
            ack_through,
            sack,
            payload: Vec::new(),
        }
    }

    /// Build a bare control frame (CONN/CONN_ACK/DISC/DISC_ACK/KEEPALIVE/NAK).
    pub fn control(
        frame_type: FrameType,
        src: Callsign,
        dst: Callsign,
        conn_id: u16,
        seq: u32,
    ) -> Self {
        Self {
            frame_type,
            flags: 0,
            src,
            dst,
            conn_id,
            seq,
            ack_through: 0,
            sack: 0,
            payload: Vec::new(),
        }
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

    /// Serialize to wire bytes (header + payload + CRC32).
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.payload.len() > LINK_MTU {
            return Err(FrameError::PayloadTooLarge {
                len: self.payload.len(),
                max: LINK_MTU,
            });
        }
        let mut b = Vec::with_capacity(LINK_OVERHEAD + self.payload.len());
        b.extend_from_slice(&MAGIC);
        b.push(VERSION);
        b.push(self.frame_type as u8);
        b.push(self.flags);
        b.extend_from_slice(&self.src.to_wire());
        b.extend_from_slice(&self.dst.to_wire());
        b.extend_from_slice(&self.conn_id.to_be_bytes());
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.ack_through.to_be_bytes());
        b.extend_from_slice(&self.sack.to_be_bytes());
        b.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        b.extend_from_slice(&self.payload);
        let crc = CRC32.checksum(&b);
        b.extend_from_slice(&crc.to_be_bytes());
        Ok(b)
    }

    /// Parse wire bytes. Exact-length and CRC-first: rejects unless the buffer is
    /// exactly `LINK_OVERHEAD + LEN` long and the CRC verifies.
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
        let len = u16::from_be_bytes([buf[39], buf[40]]) as usize;
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
        let src = Callsign::from_wire(&buf[5..15])?;
        let dst = Callsign::from_wire(&buf[15..25])?;
        let conn_id = u16::from_be_bytes([buf[25], buf[26]]);
        let seq = u32::from_be_bytes([buf[27], buf[28], buf[29], buf[30]]);
        let ack_through = u32::from_be_bytes([buf[31], buf[32], buf[33], buf[34]]);
        let sack = u32::from_be_bytes([buf[35], buf[36], buf[37], buf[38]]);
        let payload = buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        Ok(Self {
            frame_type,
            flags,
            src,
            dst,
            conn_id,
            seq,
            ack_through,
            sack,
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

    #[test]
    fn data_frame_round_trips() {
        let f = LinkFrame::data(
            call("K1ABC"),
            call("W2XYZ"),
            0x1234,
            42,
            b"hello over".to_vec(),
        );
        let bytes = f.encode().unwrap();
        assert_eq!(bytes.len(), LINK_OVERHEAD + 10);
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(f, g);
        assert_eq!(g.src.as_str(), "K1ABC");
        assert_eq!(g.payload, b"hello over");
        assert!(!g.is_end_of_over());
    }

    #[test]
    fn end_of_over_turn_token_round_trips() {
        let f = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 7, b"x".to_vec()).end_of_over();
        assert!(f.is_end_of_over());
        let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
        assert!(g.is_end_of_over());
    }

    #[test]
    fn end_of_msg_is_independent_of_end_of_over() {
        // A fragment can end a message without ending the over, and vice-versa.
        let mid =
            LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 3, b"frag".to_vec()).end_of_msg();
        assert!(mid.is_end_of_msg());
        assert!(!mid.is_end_of_over());
        let g = LinkFrame::decode(&mid.encode().unwrap()).unwrap();
        assert!(g.is_end_of_msg());
        assert!(!g.is_end_of_over());

        let both = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 4, b"last".to_vec())
            .end_of_msg()
            .end_of_over();
        assert!(both.is_end_of_msg());
        assert!(both.is_end_of_over());
    }

    #[test]
    fn ack_frame_round_trips_with_sack_and_zero_len() {
        let f = LinkFrame::ack(call("W2XYZ"), call("K1ABC"), 0x1234, 100, 0b1011);
        let bytes = f.encode().unwrap();
        let g = LinkFrame::decode(&bytes).unwrap();
        assert_eq!(g.frame_type, FrameType::Ack);
        assert_eq!(g.ack_through, 100);
        assert_eq!(g.sack, 0b1011);
        assert!(g.payload.is_empty());
        assert_eq!(u16::from_be_bytes([bytes[39], bytes[40]]), 0);
    }

    #[test]
    fn control_frames_round_trip() {
        for t in [
            FrameType::Conn,
            FrameType::ConnAck,
            FrameType::Disc,
            FrameType::DiscAck,
            FrameType::Keepalive,
            FrameType::Nak,
        ] {
            let f = LinkFrame::control(t, call("K1ABC"), call("W2XYZ"), 7, 9);
            let g = LinkFrame::decode(&f.encode().unwrap()).unwrap();
            assert_eq!(g.frame_type, t);
            assert_eq!(g.conn_id, 7);
        }
    }

    #[test]
    fn crc_rejects_a_flipped_payload_byte() {
        let f = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 1, b"payload".to_vec());
        let mut bytes = f.encode().unwrap();
        bytes[HEADER_LEN] ^= 0x01;
        assert_eq!(LinkFrame::decode(&bytes), Err(FrameError::BadCrc));
    }

    #[test]
    fn crc_rejects_a_flipped_header_byte() {
        let f = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 1, b"x".to_vec());
        let mut bytes = f.encode().unwrap();
        bytes[27] ^= 0xFF; // corrupt SEQ
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
        let f = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 1, b"abc".to_vec());
        let mut bytes = f.encode().unwrap();
        let fake = (1000u16).to_be_bytes();
        bytes[39] = fake[0];
        bytes[40] = fake[1];
        assert!(matches!(
            LinkFrame::decode(&bytes),
            Err(FrameError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn bad_magic_rejected() {
        let f = LinkFrame::control(FrameType::Conn, call("K1ABC"), call("W2XYZ"), 0, 0);
        let mut bytes = f.encode().unwrap();
        bytes[0] = 0x00;
        assert_eq!(LinkFrame::decode(&bytes), Err(FrameError::BadMagic));
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
    fn mtu_is_enforced_on_encode() {
        let f = LinkFrame::data(call("K1ABC"), call("W2XYZ"), 1, 1, vec![0u8; LINK_MTU + 1]);
        assert!(matches!(
            f.encode(),
            Err(FrameError::PayloadTooLarge { .. })
        ));
    }
}
