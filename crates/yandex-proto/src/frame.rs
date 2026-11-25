//! Binary frame codec for the Xiva messenger WebSocket channel.
//!
//! Wire layout (research: yandex-messenger-mcp `docs/protocol-research.md` §2.2 and §14.9,
//! `docs/spikes/spike4-framecodec.mjs`, `src/transport/ws/frameCodec.ts`):
//!
//! ```text
//! request / reply (type 0x01): 0x01 | msgpack [serviceIndex, reqId, method] | 0x05 + 11 bytes | JSON
//! proxy status    (type 0x02): 0x02 | msgpack [reqId, errorCode]
//! server push     (type 0x03): 0x03 | msgpack [uid, service, event, transitId] | 0x05 + 11 bytes | JSON
//! ```
//!
//! The msgpack header end is found by parsing its elements, never by scanning for `0x05`:
//! with `reqId == 5` the header itself contains a `0x05` byte (observed on a real push frame).
//! Clients send 11 zero bytes after the `0x05` marker; the server fills them with an opaque
//! value, which is ignored here.

use rmpv::Value;
use serde_json::Value as Json;

const TYPE_DATA: u8 = 0x01;
const TYPE_PROXY_STATUS: u8 = 0x02;
const TYPE_PUSH: u8 = 0x03;
const DATA_SECTION_MARKER: u8 = 0x05;
const DATA_SECTION_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("empty frame")]
    Empty,
    #[error("unknown frame type 0x{0:02x}")]
    UnknownType(u8),
    #[error("malformed msgpack header: {0}")]
    Header(String),
    #[error("data section is {0} bytes, expected at least 12")]
    ShortDataSection(usize),
    #[error("data section starts with 0x{0:02x}, expected 0x05")]
    BadSeparator(u8),
    #[error("malformed JSON body: {0}")]
    Body(#[from] serde_json::Error),
}

/// Outgoing DATA frame. Method names are not validated here.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestFrame {
    /// Transport service index; always 0 in observed traffic.
    pub service_index: u64,
    /// Per-connection counter echoed back by the server in the reply header.
    pub req_id: u64,
    pub method: String,
    pub body: Json,
}

impl RequestFrame {
    pub fn encode(&self) -> Vec<u8> {
        let header = Value::Array(vec![
            Value::from(self.service_index),
            Value::from(self.req_id),
            Value::from(self.method.as_str()),
        ]);
        let mut out = vec![TYPE_DATA];
        // Writing to a Vec cannot fail; rmpv picks the smallest uint/str encodings.
        rmpv::encode::write_value(&mut out, &header).expect("msgpack write to Vec");
        out.push(DATA_SECTION_MARKER);
        out.extend_from_slice(&[0; DATA_SECTION_LEN - 1]);
        serde_json::to_writer(&mut out, &self.body).expect("JSON write to Vec");
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IncomingFrame {
    /// DATA frame answering a request; correlate by `req_id`.
    Reply {
        service_index: u64,
        req_id: u64,
        method: String,
        /// `Json::Null` when the frame carries no JSON after the data-section prefix.
        body: Json,
    },
    /// Transport-level failure for the request `req_id` (§14.6 layer 1 error code).
    ProxyStatus { req_id: u64, error_code: u64 },
    /// Server-initiated event. `uid` and `transit_id` are kept as raw msgpack values:
    /// their wire types are not confirmed by captured traffic.
    Push {
        uid: Value,
        service: String,
        event: String,
        transit_id: Value,
        body: Json,
    },
}

impl IncomingFrame {
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let (&frame_type, mut rest) = bytes.split_first().ok_or(FrameError::Empty)?;
        if !matches!(frame_type, TYPE_DATA | TYPE_PROXY_STATUS | TYPE_PUSH) {
            return Err(FrameError::UnknownType(frame_type));
        }
        // read_value advances `rest` by exactly the parsed length: that is the header end.
        let header =
            rmpv::decode::read_value(&mut rest).map_err(|e| FrameError::Header(e.to_string()))?;
        let Value::Array(items) = header else {
            return Err(FrameError::Header("not an array".into()));
        };

        match (frame_type, items.as_slice()) {
            (TYPE_DATA, [service_index, req_id, method]) => Ok(Self::Reply {
                service_index: uint(service_index, "serviceIndex")?,
                req_id: uint(req_id, "reqId")?,
                method: string(method, "method")?,
                body: body(rest)?,
            }),
            (TYPE_PROXY_STATUS, [req_id, error_code]) => Ok(Self::ProxyStatus {
                req_id: uint(req_id, "reqId")?,
                error_code: uint(error_code, "errorCode")?,
            }),
            (TYPE_PUSH, [uid, service, event, transit_id]) => Ok(Self::Push {
                uid: uid.clone(),
                service: string(service, "service")?,
                event: string(event, "event")?,
                transit_id: transit_id.clone(),
                body: body(rest)?,
            }),
            (_, items) => Err(FrameError::Header(format!(
                "type 0x{frame_type:02x} with {} header elements",
                items.len()
            ))),
        }
    }
}

fn uint(value: &Value, name: &str) -> Result<u64, FrameError> {
    value
        .as_u64()
        .ok_or_else(|| FrameError::Header(format!("{name} is not an unsigned integer")))
}

fn string(value: &Value, name: &str) -> Result<String, FrameError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| FrameError::Header(format!("{name} is not a UTF-8 string")))
}

/// Parses the data section: 12-byte prefix, then JSON to the end of the frame.
fn body(section: &[u8]) -> Result<Json, FrameError> {
    if section.len() < DATA_SECTION_LEN {
        return Err(FrameError::ShortDataSection(section.len()));
    }
    if section[0] != DATA_SECTION_MARKER {
        return Err(FrameError::BadSeparator(section[0]));
    }
    let json = &section[DATA_SECTION_LEN..];
    if json.is_empty() {
        return Ok(Json::Null);
    }
    Ok(serde_json::from_slice(json)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Encodes a request with service index 0.
    fn request(method: &str, req_id: u64, body: Json) -> Vec<u8> {
        RequestFrame {
            service_index: 0,
            req_id,
            method: method.into(),
            body,
        }
        .encode()
    }

    #[test]
    fn encode_whoami_is_byte_exact() {
        // 01 | 93 00 01 a6 "whoami" | 05 + 11x00 | {"RequestId":"x"}
        let expected = hex(
            "01 93 00 01 a6 77686f616d69 05 0000000000 000000000000 7b2252657175657374496422 3a2278227d",
        );
        assert_eq!(request("whoami", 1, json!({"RequestId": "x"})), expected);
    }

    #[test]
    fn encode_reference_headers() {
        // Research §2.2: history seq 2, push seq 5, subscribe seq 9.
        let cases = [
            ("history", 2, "01 93 00 02 a7 686973746f7279"),
            ("push", 5, "01 93 00 05 a4 70757368"),
            ("subscribe", 9, "01 93 00 09 a9 737562736372696265"),
        ];
        for (method, req_id, header) in cases {
            let header = hex(header);
            let frame = request(method, req_id, json!({}));
            assert_eq!(&frame[..header.len()], header.as_slice(), "{method}");
            let mut data = vec![0x05];
            data.extend([0u8; 11]);
            data.extend(b"{}");
            assert_eq!(&frame[header.len()..], data.as_slice(), "{method}");
        }
    }

    #[test]
    fn encode_uint_and_str_boundaries() {
        let cases: [(u64, &str); 5] = [
            (127, "7f"),
            (128, "cc80"),
            (256, "cd0100"),
            (65536, "ce00010000"),
            (4294967296, "cf0000000100000000"),
        ];
        for (req_id, enc) in cases {
            let frame = request("m", req_id, json!(null));
            assert_eq!(&frame[..3], &[0x01, 0x93, 0x00]);
            let mut expected = hex(enc);
            expected.extend(hex("a1 6d"));
            assert_eq!(
                &frame[3..3 + expected.len()],
                expected.as_slice(),
                "{req_id}"
            );
        }
        let long = "a".repeat(40);
        let frame = request(&long, 3, json!({}));
        assert_eq!(&frame[..6], &[0x01, 0x93, 0x00, 0x03, 0xd9, 40]);
        let long = "b".repeat(300);
        let frame = request(&long, 3, json!({}));
        assert_eq!(&frame[..7], &[0x01, 0x93, 0x00, 0x03, 0xda, 0x01, 0x2c]);
    }

    #[test]
    fn decode_reply_with_server_filled_prefix() {
        // Research §2.2: whoami reply carries a 4-byte server value inside the 11 bytes.
        let mut frame = hex("01 93 00 01 a6 77686f616d69 05 00 00 00 10 da a6 0e 00 00 00 00");
        frame.extend(br#"{"RequestId":"x","Status":0}"#);
        assert_eq!(
            IncomingFrame::decode(&frame).unwrap(),
            IncomingFrame::Reply {
                service_index: 0,
                req_id: 1,
                method: "whoami".into(),
                body: json!({"RequestId": "x", "Status": 0}),
            }
        );
    }

    #[test]
    fn decode_reply_with_req_id_5_does_not_scan_for_separator() {
        let frame = request("push", 5, json!({"RequestId": "p"}));
        assert_eq!(frame.iter().position(|&b| b == 0x05), Some(3));
        let IncomingFrame::Reply {
            req_id,
            method,
            body,
            ..
        } = IncomingFrame::decode(&frame).unwrap()
        else {
            panic!("expected reply");
        };
        assert_eq!((req_id, method.as_str()), (5, "push"));
        assert_eq!(body, json!({"RequestId": "p"}));
    }

    #[test]
    fn encode_decode_roundtrip_across_uint_widths() {
        for req_id in [
            0,
            1,
            127,
            128,
            255,
            256,
            65535,
            65536,
            u32::MAX as u64,
            u64::MAX,
        ] {
            let frame = request("history", req_id, json!({"a": [1, "é"]}));
            let IncomingFrame::Reply {
                req_id: got, body, ..
            } = IncomingFrame::decode(&frame).unwrap()
            else {
                panic!("expected reply");
            };
            assert_eq!(got, req_id);
            assert_eq!(body, json!({"a": [1, "é"]}));
        }
    }

    #[test]
    fn decode_push() {
        // Synthetic: header shape from research §14.9, not captured byte-for-byte.
        let mut frame = hex("03 94 00 a9 6d657373656e676572 ab 6e65775f6d657373616765 2a");
        frame.extend(hex("05 000000000000000000 0000"));
        frame.extend(br#"{"ClientMessage":{"Plain":{}}}"#);
        assert_eq!(
            IncomingFrame::decode(&frame).unwrap(),
            IncomingFrame::Push {
                uid: Value::from(0),
                service: "messenger".into(),
                event: "new_message".into(),
                transit_id: Value::from(42),
                body: json!({"ClientMessage": {"Plain": {}}}),
            }
        );
    }

    #[test]
    fn decode_proxy_status() {
        assert_eq!(
            IncomingFrame::decode(&hex("02 92 07 06")).unwrap(),
            IncomingFrame::ProxyStatus {
                req_id: 7,
                error_code: 6
            }
        );
    }

    #[test]
    fn decode_empty_body_is_null() {
        let frame = hex("01 93 00 01 a1 6d 05 000000000000000000 0000");
        let IncomingFrame::Reply { body, .. } = IncomingFrame::decode(&frame).unwrap() else {
            panic!("expected reply");
        };
        assert_eq!(body, Json::Null);
    }

    fn assert_err(bytes: &[u8], want: fn(&FrameError) -> bool) {
        match IncomingFrame::decode(bytes) {
            Err(e) if want(&e) => {}
            other => panic!("unexpected result for {bytes:02x?}: {other:?}"),
        }
    }

    #[test]
    fn malformed_frames_return_errors() {
        assert_err(&[], |e| matches!(e, FrameError::Empty));
        assert_err(&[0x09, 0x90], |e| {
            matches!(e, FrameError::UnknownType(0x09))
        });
        // Truncated / missing header.
        assert_err(&[0x01], |e| matches!(e, FrameError::Header(_)));
        assert_err(&hex("01 93 00 01 a6 77686f"), |e| {
            matches!(e, FrameError::Header(_))
        });
        // Invalid msgpack marker (0xc1 is never used).
        assert_err(&hex("01 c1"), |e| matches!(e, FrameError::Header(_)));
        // Header is not an array.
        assert_err(&hex("01 80 00"), |e| matches!(e, FrameError::Header(_)));
        // Wrong arity for the frame type.
        assert_err(&hex("01 92 00 01"), |e| matches!(e, FrameError::Header(_)));
        assert_err(&hex("02 93 00 01 02"), |e| {
            matches!(e, FrameError::Header(_))
        });
        // Wrong element types: string reqId, integer method, negative reqId.
        assert_err(&hex("01 93 00 a1 61 a1 6d"), |e| {
            matches!(e, FrameError::Header(_))
        });
        assert_err(&hex("01 93 00 01 05"), |e| {
            matches!(e, FrameError::Header(_))
        });
        assert_err(&hex("01 93 00 ff a1 6d"), |e| {
            matches!(e, FrameError::Header(_))
        });
        // Method is not valid UTF-8: rmpv keeps it as a non-string.
        assert_err(&hex("01 93 00 01 a1 ff"), |e| {
            matches!(e, FrameError::Header(_))
        });
        // Short data section.
        assert_err(&hex("01 93 00 01 a1 6d"), |e| {
            matches!(e, FrameError::ShortDataSection(0))
        });
        assert_err(&hex("01 93 00 01 a1 6d 05 0000"), |e| {
            matches!(e, FrameError::ShortDataSection(3))
        });
        // Wrong separator.
        assert_err(
            &hex("01 93 00 01 a1 6d 07 000000000000000000 0000 7b7d"),
            |e| matches!(e, FrameError::BadSeparator(0x07)),
        );
        // Bad JSON.
        assert_err(
            &hex("01 93 00 01 a1 6d 05 000000000000000000 0000 7b"),
            |e| matches!(e, FrameError::Body(_)),
        );
        assert_err(
            &hex("03 94 00 a1 73 a1 65 01 05 000000000000000000 0000 ff"),
            |e| matches!(e, FrameError::Body(_)),
        );
    }

    #[test]
    fn every_truncation_and_bit_flip_is_handled_without_panic() {
        let mut frames = vec![
            request("history", 300, json!({"RequestId": "r", "Limit": 0})),
            hex("02 92 07 06"),
        ];
        let mut push = hex("03 94 00 a9 6d657373656e676572 ab 6e65775f6d657373616765 2a 05");
        push.extend([0u8; 11]);
        push.extend(br#"{"a":1}"#);
        frames.push(push);
        for frame in &frames {
            for end in 0..=frame.len() {
                let _ = IncomingFrame::decode(&frame[..end]);
            }
            for i in 0..frame.len() {
                for bit in 0..8 {
                    let mut copy = frame.clone();
                    copy[i] ^= 1 << bit;
                    let _ = IncomingFrame::decode(&copy);
                }
            }
        }
    }
}
