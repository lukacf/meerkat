use std::io;
use std::sync::Arc;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::types::Envelope;

use super::MAX_PAYLOAD_SIZE;

#[derive(Debug, Clone)]
pub struct EnvelopeFrame {
    pub envelope: Envelope,
    pub raw: Arc<Bytes>,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportCodec {
    max_payload_size: u32,
}

impl Default for TransportCodec {
    fn default() -> Self {
        Self {
            max_payload_size: MAX_PAYLOAD_SIZE,
        }
    }
}

impl TransportCodec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_payload_size(max_payload_size: u32) -> Self {
        Self {
            max_payload_size: max_payload_size.min(MAX_PAYLOAD_SIZE),
        }
    }
}

impl Decoder for TransportCodec {
    type Item = EnvelopeFrame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);
        if len > self.max_payload_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "message too large: {len} bytes (max {})",
                    self.max_payload_size
                ),
            ));
        }

        let frame_len = 4usize + len as usize;
        if src.len() < frame_len {
            return Ok(None);
        }

        src.advance(4);
        let raw = Arc::new(src.split_to(len as usize).freeze());

        let envelope: Envelope = ciborium::from_reader(raw.as_ref().as_ref()).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CBOR decode error: {e}"),
            )
        })?;

        Ok(Some(EnvelopeFrame { envelope, raw }))
    }
}

impl Encoder<EnvelopeFrame> for TransportCodec {
    type Error = io::Error;

    fn encode(&mut self, item: EnvelopeFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if !item.raw.is_empty() {
            let len: u32 = item.raw.len().try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "message too large to length-prefix",
                )
            })?;

            if len > self.max_payload_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "message too large: {len} bytes (max {})",
                        self.max_payload_size
                    ),
                ));
            }

            dst.reserve(4 + item.raw.len());
            dst.put_u32(len);
            dst.put_slice(item.raw.as_ref());
            return Ok(());
        }

        let frame_start = dst.len();
        dst.reserve(4);
        dst.put_u32(0);
        let payload_start = dst.len();

        let encode_result = {
            let mut writer = dst.writer();
            ciborium::into_writer(&item.envelope, &mut writer)
        };

        if let Err(err) = encode_result {
            dst.truncate(frame_start);
            return Err(io::Error::other(format!("CBOR encode error: {err}")));
        }

        let payload_len = dst.len().saturating_sub(payload_start);
        let len: u32 = payload_len.try_into().map_err(|_| {
            dst.truncate(frame_start);
            io::Error::new(
                io::ErrorKind::InvalidData,
                "message too large to length-prefix",
            )
        })?;

        if len > self.max_payload_size {
            dst.truncate(frame_start);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "message too large: {len} bytes (max {})",
                    self.max_payload_size
                ),
            ));
        }

        dst[frame_start..frame_start + 4].copy_from_slice(&len.to_be_bytes());
        Ok(())
    }
}
