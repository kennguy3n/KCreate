//! Length-prefixed framing for QUIC bidi streams.
//!
//! Each [`kcreate_collab::envelope::Envelope`] is serialised to JSON
//! and emitted as one frame on a fresh QUIC bidi stream. The frame
//! shape is intentionally trivial — QUIC already gives us framing
//! per stream, message boundaries, ordering, retransmission, and
//! congestion control — so all we need on top is the length so we
//! know how much to read before parsing JSON.
//!
//! Frame layout (network byte order):
//!
//! ```text
//! +--------+--------+--------+--------+----...----+
//! |          length (u32, big-endian)  | payload  |
//! +--------+--------+--------+--------+----...----+
//! ```
//!
//! [`MAX_FRAME_BYTES`] caps the payload at 4 MiB. The largest
//! Phase 3 envelope today is an operation broadcast carrying two
//! JSON patches; even worst-case (a paste of a 1024-node group)
//! comes in under 256 KiB. Capping at 4 MiB gives ample headroom
//! while still bounding peer memory cost when a hostile peer
//! advertises a frame length it doesn't intend to send.

use crate::error::TransportError;

/// Maximum size of a single wire frame, in bytes. Frames larger than
/// this are rejected before any allocation, to bound peer memory
/// cost in the face of a hostile or buggy sender.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Frame-length prefix size in bytes.
const LEN_PREFIX: usize = std::mem::size_of::<u32>();

/// Encode `payload` into a wire frame (length-prefixed JSON).
///
/// Returns `Err(FrameTooLarge)` if the payload exceeds
/// [`MAX_FRAME_BYTES`].
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, TransportError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(TransportError::FrameTooLarge {
            size: payload.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let mut buf = Vec::with_capacity(LEN_PREFIX + payload.len());
    buf.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    buf.extend_from_slice(payload);
    Ok(buf)
}

/// Decode the length prefix at the start of `buf`. Returns
/// `(payload_len, header_size)` so the caller can slice the rest
/// out of its own buffer.
pub fn decode_frame(buf: &[u8]) -> Result<(usize, usize), TransportError> {
    if buf.len() < LEN_PREFIX {
        return Err(TransportError::Malformed(format!(
            "frame header truncated ({} bytes, need {})",
            buf.len(),
            LEN_PREFIX
        )));
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(TransportError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_BYTES,
        });
    }
    Ok((len, LEN_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_small_frame() {
        let payload = b"hello, kcreate";
        let encoded = encode_frame(payload).expect("encode");
        let (len, header) = decode_frame(&encoded).expect("decode header");
        assert_eq!(len, payload.len());
        assert_eq!(&encoded[header..], payload);
    }

    #[test]
    fn rejects_oversized_payload() {
        let payload = vec![0u8; MAX_FRAME_BYTES + 1];
        let err = encode_frame(&payload).expect_err("must reject");
        match err {
            TransportError::FrameTooLarge { size, max } => {
                assert_eq!(size, MAX_FRAME_BYTES + 1);
                assert_eq!(max, MAX_FRAME_BYTES);
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_oversized_length_prefix() {
        // Craft a malicious header claiming an enormous length.
        let mut header = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes().to_vec();
        header.extend_from_slice(b"x"); // any tail
        let err = decode_frame(&header).expect_err("must reject");
        assert!(matches!(err, TransportError::FrameTooLarge { .. }));
    }

    #[test]
    fn rejects_truncated_header() {
        let err = decode_frame(&[0u8, 0u8]).expect_err("must reject");
        assert!(matches!(err, TransportError::Malformed(_)));
    }
}
