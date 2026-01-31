#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use uuid::Uuid;

use meerkat_comms::{
    Envelope, EnvelopeFrame, Keypair, MessageKind, PubKey, Signature, TransportCodec,
    transport::MAX_PAYLOAD_SIZE,
};

fn make_test_envelope(body: String) -> Envelope {
    let keypair = Keypair::generate();
    let mut envelope = Envelope {
        id: Uuid::new_v4(),
        from: keypair.public_key(),
        to: PubKey::new([2u8; 32]),
        kind: MessageKind::Message { body },
        sig: Signature::new([0u8; 64]),
    };
    envelope.sign(&keypair);
    envelope
}

#[test]
fn test_rct_contracts_envelope_signable_bytes_are_canonical() {
    let envelope = Envelope {
        id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        from: PubKey::new([1u8; 32]),
        to: PubKey::new([2u8; 32]),
        kind: MessageKind::Message {
            body: "hello".to_string(),
        },
        sig: Signature::new([0u8; 64]),
    };

    let bytes = envelope.signable_bytes();

    let mut expected = Vec::new();
    expected.extend_from_slice(&[
        0x84, // array(4)
        0x50, // bytes(16)
        0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00, // uuid bytes
        0x58, 0x20, // bytes(32) from
    ]);
    expected.extend(std::iter::repeat_n(0x01, 32));
    expected.extend_from_slice(&[0x58, 0x20]); // bytes(32) to
    expected.extend(std::iter::repeat_n(0x02, 32));
    expected.extend_from_slice(&[
        0xa2, // map(2)
        0x64, b'b', b'o', b'd', b'y', // "body"
        0x65, b'h', b'e', b'l', b'l', b'o', // "hello"
        0x64, b't', b'y', b'p', b'e', // "type"
        0x67, b'm', b'e', b's', b's', b'a', b'g', b'e', // "message"
    ]);

    assert_eq!(bytes, expected);
}

#[test]
fn test_rct_contracts_transport_codec_roundtrip() {
    let envelope = make_test_envelope("hello".to_string());
    assert!(envelope.verify());

    let frame = EnvelopeFrame {
        envelope: envelope.clone(),
        raw: Arc::new(Bytes::new()),
    };

    let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
    let mut dst = BytesMut::new();
    codec.encode(frame, &mut dst).unwrap();

    let declared_len = u32::from_be_bytes(dst[..4].try_into().unwrap());
    assert_eq!(declared_len as usize, dst.len() - 4);

    let mut src = dst.clone();
    let decoded = codec.decode(&mut src).unwrap().expect("frame present");
    assert_eq!(decoded.envelope, envelope);
    assert_eq!(decoded.raw.as_ref().as_ref(), &dst[4..]);
}

#[test]
fn test_rct_contracts_transport_codec_rejects_oversize_len_prefix_on_decode() {
    let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
    let mut src = BytesMut::new();
    src.put_u32(MAX_PAYLOAD_SIZE + 1);

    let err = codec.decode(&mut src).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn test_rct_contracts_transport_codec_rejects_oversize_payload_on_encode() {
    let oversize = "a".repeat(MAX_PAYLOAD_SIZE as usize + 1024);
    let envelope = make_test_envelope(oversize);

    let frame = EnvelopeFrame {
        envelope,
        raw: Arc::new(Bytes::new()),
    };

    let mut codec = TransportCodec::new(MAX_PAYLOAD_SIZE);
    let mut dst = BytesMut::new();
    let err = codec.encode(frame, &mut dst).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}
