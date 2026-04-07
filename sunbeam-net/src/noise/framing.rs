use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Record frame type (controlbase post-handshake data).
pub const FRAME_TYPE_DATA: u8 = 0x04;
/// Control frame type.
pub const FRAME_TYPE_CONTROL: u8 = 0x01;
/// Maximum payload size per frame.
pub const MAX_FRAME_SIZE: usize = 4096;
/// Size of the frame header (1 byte type + 2 bytes length).
pub const HEADER_SIZE: usize = 3;

/// A single Noise frame (type + payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoiseFrame {
    pub frame_type: u8,
    pub payload: BytesMut,
}

/// Codec for encoding/decoding Noise frames on a byte stream.
#[derive(Debug, Default)]
pub struct NoiseFrameCodec;

impl Decoder for NoiseFrameCodec {
    type Item = NoiseFrame;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> std::result::Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_SIZE {
            return Ok(None);
        }

        let frame_type = src[0];
        let payload_len = u16::from_be_bytes([src[1], src[2]]) as usize;

        if payload_len > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("frame payload {payload_len} exceeds maximum {MAX_FRAME_SIZE}"),
            ));
        }

        let total = HEADER_SIZE + payload_len;
        if src.len() < total {
            // Reserve space so the next read can fill it.
            src.reserve(total - src.len());
            return Ok(None);
        }

        src.advance(HEADER_SIZE);
        let payload = src.split_to(payload_len);

        Ok(Some(NoiseFrame {
            frame_type,
            payload,
        }))
    }
}

impl Encoder<NoiseFrame> for NoiseFrameCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: NoiseFrame, dst: &mut BytesMut) -> std::result::Result<(), Self::Error> {
        if item.payload.len() > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "payload length {} exceeds maximum {MAX_FRAME_SIZE}",
                    item.payload.len()
                ),
            ));
        }

        let len = item.payload.len() as u16;
        dst.reserve(HEADER_SIZE + item.payload.len());
        dst.put_u8(item.frame_type);
        dst.put_u16(len);
        dst.put(item.payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame_type: u8, payload: &[u8]) {
        let mut codec = NoiseFrameCodec;
        let frame = NoiseFrame {
            frame_type,
            payload: BytesMut::from(payload),
        };

        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();

        let decoded = codec.decode(&mut buf).unwrap().expect("should decode a frame");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trip_data_frame() {
        round_trip(FRAME_TYPE_DATA, b"hello world");
    }

    #[test]
    fn round_trip_control_frame() {
        round_trip(FRAME_TYPE_CONTROL, b"\x01\x02\x03");
    }

    #[test]
    fn round_trip_empty_payload() {
        round_trip(FRAME_TYPE_DATA, b"");
    }

    #[test]
    fn round_trip_max_size() {
        let payload = vec![0xAB; MAX_FRAME_SIZE];
        round_trip(FRAME_TYPE_DATA, &payload);
    }

    #[test]
    fn partial_header_returns_none() {
        let mut codec = NoiseFrameCodec;
        let mut buf = BytesMut::from(&[0x00, 0x00][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn partial_payload_returns_none() {
        let mut codec = NoiseFrameCodec;
        // Header says 10 bytes of payload, but only 3 provided.
        let mut buf = BytesMut::from(&[0x00, 0x00, 0x0A, 0x01, 0x02, 0x03][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn oversized_payload_is_rejected_on_decode() {
        let mut codec = NoiseFrameCodec;
        let bad_len = (MAX_FRAME_SIZE as u16) + 1;
        let mut buf = BytesMut::new();
        buf.put_u8(0x00);
        buf.put_u16(bad_len);
        buf.extend_from_slice(&vec![0; bad_len as usize]);
        assert!(codec.decode(&mut buf).is_err());
    }

    #[test]
    fn oversized_payload_is_rejected_on_encode() {
        let mut codec = NoiseFrameCodec;
        let frame = NoiseFrame {
            frame_type: FRAME_TYPE_DATA,
            payload: BytesMut::from(&vec![0; MAX_FRAME_SIZE + 1][..]),
        };
        let mut buf = BytesMut::new();
        assert!(codec.encode(frame, &mut buf).is_err());
    }

    #[test]
    fn multiple_frames_in_buffer() {
        let mut codec = NoiseFrameCodec;
        let mut buf = BytesMut::new();

        let f1 = NoiseFrame {
            frame_type: FRAME_TYPE_DATA,
            payload: BytesMut::from(&b"aaa"[..]),
        };
        let f2 = NoiseFrame {
            frame_type: FRAME_TYPE_CONTROL,
            payload: BytesMut::from(&b"bbb"[..]),
        };
        codec.encode(f1.clone(), &mut buf).unwrap();
        codec.encode(f2.clone(), &mut buf).unwrap();

        let d1 = codec.decode(&mut buf).unwrap().unwrap();
        let d2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(d1, f1);
        assert_eq!(d2, f2);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }
}
