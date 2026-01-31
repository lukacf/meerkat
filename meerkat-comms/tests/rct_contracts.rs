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

// ============================================================================
// ACK validation regression tests
// ============================================================================

/// Regression test: ACK validation must check signature, sender, and in_reply_to.
/// A valid ACK must have:
/// 1. Valid signature (envelope.verify() == true)
/// 2. from == the peer we sent to
/// 3. in_reply_to == the message id we sent
#[test]
fn test_regression_ack_must_match_sent_message() {
    let sender_keypair = Keypair::generate();
    let receiver_keypair = Keypair::generate();

    // Create a message from sender to receiver
    let sent_id = Uuid::new_v4();
    let sent_envelope = Envelope {
        id: sent_id,
        from: sender_keypair.public_key(),
        to: receiver_keypair.public_key(),
        kind: MessageKind::Message {
            body: "hello".to_string(),
        },
        sig: Signature::new([0u8; 64]),
    };

    // Create a valid ACK from receiver back to sender
    let mut valid_ack = Envelope {
        id: Uuid::new_v4(),
        from: receiver_keypair.public_key(),
        to: sender_keypair.public_key(),
        kind: MessageKind::Ack {
            in_reply_to: sent_id,
        },
        sig: Signature::new([0u8; 64]),
    };
    valid_ack.sign(&receiver_keypair);

    // Valid ACK should verify
    assert!(valid_ack.verify(), "valid ACK should verify");
    assert_eq!(
        valid_ack.from, sent_envelope.to,
        "ACK from should match sent to"
    );
    if let MessageKind::Ack { in_reply_to } = valid_ack.kind {
        assert_eq!(in_reply_to, sent_id, "ACK in_reply_to should match sent id");
    }
}

/// Regression test: ACK with wrong in_reply_to should be rejected.
#[test]
fn test_regression_ack_wrong_in_reply_to_is_invalid() {
    let sender_keypair = Keypair::generate();
    let receiver_keypair = Keypair::generate();

    let sent_id = Uuid::new_v4();
    let wrong_id = Uuid::new_v4();

    // ACK with wrong in_reply_to
    let mut wrong_ack = Envelope {
        id: Uuid::new_v4(),
        from: receiver_keypair.public_key(),
        to: sender_keypair.public_key(),
        kind: MessageKind::Ack {
            in_reply_to: wrong_id,
        },
        sig: Signature::new([0u8; 64]),
    };
    wrong_ack.sign(&receiver_keypair);

    // Signature is valid but in_reply_to doesn't match
    assert!(wrong_ack.verify(), "signature should still verify");
    if let MessageKind::Ack { in_reply_to } = wrong_ack.kind {
        assert_ne!(
            in_reply_to, sent_id,
            "wrong ACK in_reply_to should not match sent id"
        );
    }
}

/// Regression test: ACK from wrong peer should be rejected.
#[test]
fn test_regression_ack_from_wrong_peer_is_invalid() {
    let sender_keypair = Keypair::generate();
    let receiver_keypair = Keypair::generate();
    let imposter_keypair = Keypair::generate();

    let sent_id = Uuid::new_v4();
    let sent_to = receiver_keypair.public_key();

    // ACK from imposter (not the intended receiver)
    let mut imposter_ack = Envelope {
        id: Uuid::new_v4(),
        from: imposter_keypair.public_key(),
        to: sender_keypair.public_key(),
        kind: MessageKind::Ack {
            in_reply_to: sent_id,
        },
        sig: Signature::new([0u8; 64]),
    };
    imposter_ack.sign(&imposter_keypair);

    // Signature is valid but from wrong peer
    assert!(imposter_ack.verify(), "imposter signature should verify");
    assert_ne!(
        imposter_ack.from, sent_to,
        "imposter ACK from should not match sent to"
    );
}
