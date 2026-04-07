use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Size of the frame header (1 byte type + 4 bytes big-endian length).
pub const HEADER_SIZE: usize = 5;
/// Maximum payload size per frame (64KB).
pub const MAX_FRAME_SIZE: usize = 65535;

// Frame type constants.
pub const FRAME_SERVER_KEY: u8 = 0x01;
pub const FRAME_CLIENT_INFO: u8 = 0x02;
pub const FRAME_SERVER_INFO: u8 = 0x03;
pub const FRAME_SEND_PACKET: u8 = 0x04;
pub const FRAME_RECV_PACKET: u8 = 0x05;
pub const FRAME_KEEP_ALIVE: u8 = 0x06;
pub const FRAME_NOTE_PREFERRED: u8 = 0x07;
pub const FRAME_PEER_GONE: u8 = 0x08;
pub const FRAME_PEER_PRESENT: u8 = 0x09;
pub const FRAME_WATCH_CONNS: u8 = 0x0a;
pub const FRAME_CLOSE_PEER: u8 = 0x0b;
pub const FRAME_PING: u8 = 0x0c;
pub const FRAME_PONG: u8 = 0x0d;
pub const FRAME_HEALTH: u8 = 0x0e;
pub const FRAME_RESTARTING: u8 = 0x0f;

/// A DERP protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerpFrame {
    pub frame_type: u8,
    pub payload: BytesMut,
}

/// Codec for encoding/decoding DERP frames on a byte stream.
#[derive(Debug, Default)]
pub struct DerpFrameCodec;

impl Decoder for DerpFrameCodec {
    type Item = DerpFrame;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::result::Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_SIZE {
            return Ok(None);
        }

        let frame_type = src[0];
        let payload_len = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;

        if payload_len > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("frame payload {payload_len} exceeds maximum {MAX_FRAME_SIZE}"),
            ));
        }

        let total = HEADER_SIZE + payload_len;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }

        src.advance(HEADER_SIZE);
        let payload = src.split_to(payload_len);

        Ok(Some(DerpFrame {
            frame_type,
            payload,
        }))
    }
}

impl Encoder<DerpFrame> for DerpFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: DerpFrame, dst: &mut BytesMut) -> std::result::Result<(), Self::Error> {
        if item.payload.len() > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "payload length {} exceeds maximum {MAX_FRAME_SIZE}",
                    item.payload.len()
                ),
            ));
        }

        let len = item.payload.len() as u32;
        dst.reserve(HEADER_SIZE + item.payload.len());
        dst.put_u8(item.frame_type);
        dst.put_u32(len);
        dst.put(item.payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame_type: u8, payload: &[u8]) {
        let mut codec = DerpFrameCodec;
        let frame = DerpFrame {
            frame_type,
            payload: BytesMut::from(payload),
        };

        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();

        let decoded = codec.decode(&mut buf).unwrap().expect("should decode a frame");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_round_trip_send_packet() {
        let mut payload = BytesMut::new();
        payload.extend_from_slice(&[0xAA; 32]); // dest key
        payload.extend_from_slice(b"wireguard packet data");
        round_trip(FRAME_SEND_PACKET, &payload);
    }

    #[test]
    fn test_round_trip_recv_packet() {
        let mut payload = BytesMut::new();
        payload.extend_from_slice(&[0xBB; 32]); // source key
        payload.extend_from_slice(b"received wg data");
        round_trip(FRAME_RECV_PACKET, &payload);
    }

    #[test]
    fn test_round_trip_keep_alive() {
        round_trip(FRAME_KEEP_ALIVE, b"");
    }

    #[test]
    fn test_round_trip_ping() {
        round_trip(FRAME_PING, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
    }

    #[test]
    fn test_round_trip_max_size() {
        let payload = vec![0xCD; MAX_FRAME_SIZE];
        round_trip(FRAME_SEND_PACKET, &payload);
    }

    #[test]
    fn test_partial_header_returns_none() {
        let mut codec = DerpFrameCodec;
        // Only 4 bytes, need 5 for header
        let mut buf = BytesMut::from(&[0x04, 0x00, 0x00, 0x00][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_partial_payload_returns_none() {
        let mut codec = DerpFrameCodec;
        // Header says 10 bytes of payload, but only 3 provided
        let mut buf = BytesMut::new();
        buf.put_u8(FRAME_SEND_PACKET);
        buf.put_u32(10);
        buf.extend_from_slice(&[0x01, 0x02, 0x03]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_oversized_rejected() {
        let mut codec = DerpFrameCodec;
        let bad_len = (MAX_FRAME_SIZE as u32) + 1;
        let mut buf = BytesMut::new();
        buf.put_u8(FRAME_SEND_PACKET);
        buf.put_u32(bad_len);
        buf.extend_from_slice(&vec![0; bad_len as usize]);
        assert!(codec.decode(&mut buf).is_err());

        // Also test encode rejection
        let mut codec2 = DerpFrameCodec;
        let frame = DerpFrame {
            frame_type: FRAME_SEND_PACKET,
            payload: BytesMut::from(&vec![0; MAX_FRAME_SIZE + 1][..]),
        };
        let mut dst = BytesMut::new();
        assert!(codec2.encode(frame, &mut dst).is_err());
    }

    #[test]
    fn test_multiple_frames_in_buffer() {
        let mut codec = DerpFrameCodec;
        let mut buf = BytesMut::new();

        let f1 = DerpFrame {
            frame_type: FRAME_SEND_PACKET,
            payload: BytesMut::from(&b"hello"[..]),
        };
        let f2 = DerpFrame {
            frame_type: FRAME_KEEP_ALIVE,
            payload: BytesMut::new(),
        };
        let f3 = DerpFrame {
            frame_type: FRAME_RECV_PACKET,
            payload: BytesMut::from(&b"world"[..]),
        };

        codec.encode(f1.clone(), &mut buf).unwrap();
        codec.encode(f2.clone(), &mut buf).unwrap();
        codec.encode(f3.clone(), &mut buf).unwrap();

        let d1 = codec.decode(&mut buf).unwrap().unwrap();
        let d2 = codec.decode(&mut buf).unwrap().unwrap();
        let d3 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(d1, f1);
        assert_eq!(d2, f2);
        assert_eq!(d3, f3);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }
}
