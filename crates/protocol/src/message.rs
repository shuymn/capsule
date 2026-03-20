//! Typed messages for the capsule wire protocol.
//!
//! The wire format is a sequence of netstring-encoded fields terminated by `\n`.
//! Each message type has a fixed field order and a type discriminator.

use crate::{ProtocolError, netstring};

/// Protocol version for v1.
pub const PROTOCOL_VERSION: u8 = 1;

const TYPE_REQUEST: &[u8] = b"Q";
const TYPE_RENDER_RESULT: &[u8] = b"R";
const TYPE_UPDATE: &[u8] = b"U";
const TYPE_HELLO: &[u8] = b"H";
const TYPE_HELLO_ACK: &[u8] = b"A";

/// An 8-byte session identifier, displayed as 16 hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId([u8; 8]);

impl SessionId {
    /// Create a `SessionId` from raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// Return the underlying bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    /// Parse a `SessionId` from 16 hex characters.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidField`] if `hex` is not exactly 16 valid hex characters.
    pub fn from_hex(hex: &[u8]) -> Result<Self, ProtocolError> {
        if hex.len() != 16 {
            return Err(ProtocolError::InvalidField {
                field: "session_id",
                reason: "must be 16 hex characters",
            });
        }
        let mut bytes = [0u8; 8];
        for (i, chunk) in hex.chunks_exact(2).enumerate() {
            let hi = hex_digit(chunk[0])?;
            let lo = hex_digit(chunk[1])?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Self(bytes))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

const fn hex_digit(b: u8) -> Result<u8, ProtocolError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(ProtocolError::InvalidField {
            field: "session_id",
            reason: "invalid hex digit",
        }),
    }
}

/// A prompt request from zsh to the daemon.
///
/// Wire type: `Q` (10 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Protocol version (always 1 for v1).
    pub version: u8,
    /// Session identifier.
    pub session_id: SessionId,
    /// Monotonically increasing generation counter.
    pub generation: u64,
    /// Current working directory.
    pub cwd: String,
    /// Terminal width in columns.
    pub cols: u16,
    /// Exit code of the last command.
    pub last_exit_code: i32,
    /// Duration of the last command in milliseconds, if available.
    pub duration_ms: Option<u64>,
    /// Current zle keymap name.
    pub keymap: String,
}

/// Immediate response from the daemon with fast module outputs.
///
/// Wire type: `R` (8 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderResult {
    /// Protocol version.
    pub version: u8,
    /// Session identifier.
    pub session_id: SessionId,
    /// Generation this response corresponds to.
    pub generation: u64,
    /// Info line (line 1 of the prompt).
    pub left1: String,
    /// Input line (line 2 of the prompt).
    pub left2: String,
}

/// Deferred update after slow modules complete.
///
/// Wire type: `U` (8 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// Protocol version.
    pub version: u8,
    /// Session identifier.
    pub session_id: SessionId,
    /// Generation this update corresponds to.
    pub generation: u64,
    /// Updated info line.
    pub left1: String,
    /// Updated input line.
    pub left2: String,
}

/// Build ID handshake: client → daemon.
///
/// Wire type: `H` (3 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Protocol version.
    pub version: u8,
    /// Binary fingerprint of the sender.
    pub build_id: String,
}

/// Build ID handshake: daemon → client.
///
/// Wire type: `A` (3 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloAck {
    /// Protocol version.
    pub version: u8,
    /// Binary fingerprint of the daemon.
    pub build_id: String,
}

/// Any message on the wire.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A prompt request.
    Request(Request),
    /// An immediate render result.
    RenderResult(RenderResult),
    /// A deferred update.
    Update(Update),
    /// A build ID handshake request.
    Hello(Hello),
    /// A build ID handshake acknowledgement.
    HelloAck(HelloAck),
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Write the common header fields (version, type, `session_id`, generation) into `buf`.
fn encode_header(buf: &mut Vec<u8>, version: u8, type_tag: &[u8], sid: SessionId, generation: u64) {
    netstring::encode_into(buf, version.to_string().as_bytes());
    netstring::encode_into(buf, type_tag);
    netstring::encode_into(buf, sid.to_string().as_bytes());
    netstring::encode_into(buf, generation.to_string().as_bytes());
}

impl Request {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        encode_header(
            &mut buf,
            self.version,
            TYPE_REQUEST,
            self.session_id,
            self.generation,
        );
        netstring::encode_into(&mut buf, self.cwd.as_bytes());
        netstring::encode_into(&mut buf, self.cols.to_string().as_bytes());
        netstring::encode_into(&mut buf, self.last_exit_code.to_string().as_bytes());
        match self.duration_ms {
            Some(d) => netstring::encode_into(&mut buf, d.to_string().as_bytes()),
            None => netstring::encode_into(&mut buf, b""),
        }
        netstring::encode_into(&mut buf, self.keymap.as_bytes());
        netstring::encode_into(&mut buf, b""); // meta (reserved)
        buf
    }
}

impl RenderResult {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        encode_header(
            &mut buf,
            self.version,
            TYPE_RENDER_RESULT,
            self.session_id,
            self.generation,
        );
        netstring::encode_into(&mut buf, self.left1.as_bytes());
        netstring::encode_into(&mut buf, self.left2.as_bytes());
        netstring::encode_into(&mut buf, b""); // right1 (reserved)
        netstring::encode_into(&mut buf, b""); // meta (reserved)
        buf
    }
}

impl Update {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        encode_header(
            &mut buf,
            self.version,
            TYPE_UPDATE,
            self.session_id,
            self.generation,
        );
        netstring::encode_into(&mut buf, self.left1.as_bytes());
        netstring::encode_into(&mut buf, self.left2.as_bytes());
        netstring::encode_into(&mut buf, b""); // right1 (reserved)
        netstring::encode_into(&mut buf, b""); // meta (reserved)
        buf
    }
}

impl Hello {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        netstring::encode_into(&mut buf, self.version.to_string().as_bytes());
        netstring::encode_into(&mut buf, TYPE_HELLO);
        netstring::encode_into(&mut buf, self.build_id.as_bytes());
        buf
    }
}

impl HelloAck {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        netstring::encode_into(&mut buf, self.version.to_string().as_bytes());
        netstring::encode_into(&mut buf, TYPE_HELLO_ACK);
        netstring::encode_into(&mut buf, self.build_id.as_bytes());
        buf
    }
}

impl Message {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        match self {
            Self::Request(r) => r.to_wire(),
            Self::RenderResult(r) => r.to_wire(),
            Self::Update(u) => u.to_wire(),
            Self::Hello(h) => h.to_wire(),
            Self::HelloAck(a) => a.to_wire(),
        }
    }

    /// Deserialize from wire bytes (without trailing LF).
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] if the input cannot be parsed as a valid message.
    pub fn from_wire(input: &[u8]) -> Result<Self, ProtocolError> {
        let fields = decode_all_fields(input)?;

        if fields.len() < 2 {
            return Err(ProtocolError::WrongFieldCount {
                expected: 2,
                got: fields.len(),
            });
        }

        let version = parse_field::<u8>(fields[0], "version")?;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidField {
                field: "version",
                reason: "unsupported protocol version",
            });
        }

        match fields[1] {
            TYPE_REQUEST => Ok(Self::Request(Request::from_fields(version, &fields)?)),
            TYPE_RENDER_RESULT => Ok(Self::RenderResult(RenderResult::from_fields(
                version, &fields,
            )?)),
            TYPE_UPDATE => Ok(Self::Update(Update::from_fields(version, &fields)?)),
            TYPE_HELLO => Ok(Self::Hello(Hello::from_fields(version, &fields)?)),
            TYPE_HELLO_ACK => Ok(Self::HelloAck(HelloAck::from_fields(version, &fields)?)),
            _ => Err(ProtocolError::UnknownMessageType),
        }
    }
}

// ---------------------------------------------------------------------------
// Deserialization helpers
// ---------------------------------------------------------------------------

fn decode_all_fields(mut input: &[u8]) -> Result<Vec<&[u8]>, ProtocolError> {
    let mut fields = Vec::with_capacity(10);
    while !input.is_empty() {
        let (data, rest) = netstring::decode(input)?;
        fields.push(data);
        input = rest;
    }
    Ok(fields)
}

fn parse_field<T: std::str::FromStr>(field: &[u8], name: &'static str) -> Result<T, ProtocolError> {
    let s = std::str::from_utf8(field).map_err(|_e| ProtocolError::InvalidField {
        field: name,
        reason: "not utf-8",
    })?;
    s.parse().map_err(|_e| ProtocolError::InvalidField {
        field: name,
        reason: "not a valid number",
    })
}

fn parse_opt_u64(field: &[u8], name: &'static str) -> Result<Option<u64>, ProtocolError> {
    if field.is_empty() {
        return Ok(None);
    }
    parse_field::<u64>(field, name).map(Some)
}

fn field_to_string(field: &[u8], name: &'static str) -> Result<String, ProtocolError> {
    std::str::from_utf8(field)
        .map(ToOwned::to_owned)
        .map_err(|_e| ProtocolError::InvalidField {
            field: name,
            reason: "not utf-8",
        })
}

impl Request {
    const FIELD_COUNT: usize = 10;

    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            session_id: SessionId::from_hex(fields[2])?,
            generation: parse_field::<u64>(fields[3], "generation")?,
            cwd: field_to_string(fields[4], "cwd")?,
            cols: parse_field::<u16>(fields[5], "cols")?,
            last_exit_code: parse_field::<i32>(fields[6], "last_exit_code")?,
            duration_ms: parse_opt_u64(fields[7], "duration_ms")?,
            keymap: field_to_string(fields[8], "keymap")?,
            // fields[9] = meta (ignored)
        })
    }
}

impl RenderResult {
    const FIELD_COUNT: usize = 8;

    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            session_id: SessionId::from_hex(fields[2])?,
            generation: parse_field::<u64>(fields[3], "generation")?,
            left1: field_to_string(fields[4], "left1")?,
            left2: field_to_string(fields[5], "left2")?,
            // fields[6] = right1 (ignored)
            // fields[7] = meta (ignored)
        })
    }
}

impl Update {
    const FIELD_COUNT: usize = 8;

    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            session_id: SessionId::from_hex(fields[2])?,
            generation: parse_field::<u64>(fields[3], "generation")?,
            left1: field_to_string(fields[4], "left1")?,
            left2: field_to_string(fields[5], "left2")?,
            // fields[6] = right1 (ignored)
            // fields[7] = meta (ignored)
        })
    }
}

impl Hello {
    const FIELD_COUNT: usize = 3;

    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            build_id: field_to_string(fields[2], "build_id")?,
        })
    }
}

impl HelloAck {
    const FIELD_COUNT: usize = 3;

    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            build_id: field_to_string(fields[2], "build_id")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session_id() -> SessionId {
        SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
    }

    fn sample_request() -> Request {
        Request {
            version: PROTOCOL_VERSION,
            session_id: sample_session_id(),
            generation: 42,
            cwd: "/home/user/project".to_owned(),
            cols: 120,
            last_exit_code: 0,
            duration_ms: Some(1500),
            keymap: "main".to_owned(),
        }
    }

    fn sample_render_result() -> RenderResult {
        RenderResult {
            version: PROTOCOL_VERSION,
            session_id: sample_session_id(),
            generation: 42,
            left1: "~/project  main".to_owned(),
            left2: "❯ ".to_owned(),
        }
    }

    fn sample_update() -> Update {
        Update {
            version: PROTOCOL_VERSION,
            session_id: sample_session_id(),
            generation: 42,
            left1: "~/project  main *2".to_owned(),
            left2: "❯ ".to_owned(),
        }
    }

    fn sample_hello() -> Hello {
        Hello {
            version: PROTOCOL_VERSION,
            build_id: "12345:1700000000000000000".to_owned(),
        }
    }

    fn sample_hello_ack() -> HelloAck {
        HelloAck {
            version: PROTOCOL_VERSION,
            build_id: "12345:1700000000000000000".to_owned(),
        }
    }

    // -- SessionId --

    #[test]
    fn test_session_id_hex_round_trip() -> Result<(), ProtocolError> {
        let sid = sample_session_id();
        let hex = sid.to_string();
        assert_eq!(hex, "0123456789abcdef");

        let parsed = SessionId::from_hex(hex.as_bytes())?;
        assert_eq!(parsed, sid);
        Ok(())
    }

    #[test]
    fn test_session_id_uppercase_hex() -> Result<(), ProtocolError> {
        let parsed = SessionId::from_hex(b"0123456789ABCDEF")?;
        assert_eq!(parsed, sample_session_id());
        Ok(())
    }

    #[test]
    fn test_session_id_invalid_length() {
        let result = SessionId::from_hex(b"0123");
        assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
    }

    #[test]
    fn test_session_id_invalid_hex() {
        let result = SessionId::from_hex(b"012345678XABCDEF");
        assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
    }

    // -- Request round-trip --

    #[test]
    fn test_request_round_trip() -> Result<(), ProtocolError> {
        let req = sample_request();
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_with_none_duration() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.duration_ms = None;
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_with_negative_exit_code() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.last_exit_code = -1;
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_with_utf8_cwd() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.cwd = "/home/ユーザー/プロジェクト".to_owned();
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    // -- RenderResult round-trip --

    #[test]
    fn test_render_result_round_trip() -> Result<(), ProtocolError> {
        let rr = sample_render_result();
        let wire = rr.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::RenderResult(rr));
        Ok(())
    }

    #[test]
    fn test_render_result_empty_prompts() -> Result<(), ProtocolError> {
        let rr = RenderResult {
            version: PROTOCOL_VERSION,
            session_id: sample_session_id(),
            generation: 0,
            left1: String::new(),
            left2: String::new(),
        };
        let wire = rr.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::RenderResult(rr));
        Ok(())
    }

    // -- Update round-trip --

    #[test]
    fn test_update_round_trip() -> Result<(), ProtocolError> {
        let upd = sample_update();
        let wire = upd.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Update(upd));
        Ok(())
    }

    // -- Hello round-trip --

    #[test]
    fn test_hello_round_trip() -> Result<(), ProtocolError> {
        let hello = sample_hello();
        let wire = hello.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Hello(hello));
        Ok(())
    }

    #[test]
    fn test_hello_empty_build_id() -> Result<(), ProtocolError> {
        let hello = Hello {
            version: PROTOCOL_VERSION,
            build_id: String::new(),
        };
        let wire = hello.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Hello(hello));
        Ok(())
    }

    // -- HelloAck round-trip --

    #[test]
    fn test_hello_ack_round_trip() -> Result<(), ProtocolError> {
        let ack = sample_hello_ack();
        let wire = ack.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::HelloAck(ack));
        Ok(())
    }

    // -- Error cases --

    #[test]
    fn test_from_wire_empty_input() {
        let result = Message::from_wire(b"");
        assert!(matches!(
            result,
            Err(ProtocolError::WrongFieldCount {
                expected: 2,
                got: 0
            })
        ));
    }

    #[test]
    fn test_from_wire_unknown_type() {
        // Build: version=1, type=X
        let mut wire = netstring::encode(b"1");
        wire.extend_from_slice(&netstring::encode(b"X"));
        let result = Message::from_wire(&wire);
        assert!(matches!(result, Err(ProtocolError::UnknownMessageType)));
    }

    #[test]
    fn test_from_wire_wrong_field_count() {
        // Build a Q message with only 5 fields instead of 10
        let mut wire = netstring::encode(b"1");
        wire.extend_from_slice(&netstring::encode(b"Q"));
        wire.extend_from_slice(&netstring::encode(b"0123456789abcdef"));
        wire.extend_from_slice(&netstring::encode(b"1"));
        wire.extend_from_slice(&netstring::encode(b"/tmp"));
        let result = Message::from_wire(&wire);
        assert!(matches!(
            result,
            Err(ProtocolError::WrongFieldCount { expected: 10, .. })
        ));
    }

    #[test]
    fn test_from_wire_invalid_generation() {
        let mut req = sample_request();
        req.generation = 0;
        let mut wire = req.to_wire();
        // Corrupt the generation field: replace "0" with "abc"
        // Easier: build manually with invalid generation
        wire.clear();
        wire.extend_from_slice(&netstring::encode(b"1"));
        wire.extend_from_slice(&netstring::encode(b"Q"));
        wire.extend_from_slice(&netstring::encode(b"0123456789abcdef"));
        wire.extend_from_slice(&netstring::encode(b"not_a_number"));
        wire.extend_from_slice(&netstring::encode(b"/tmp"));
        wire.extend_from_slice(&netstring::encode(b"80"));
        wire.extend_from_slice(&netstring::encode(b"0"));
        wire.extend_from_slice(&netstring::encode(b""));
        wire.extend_from_slice(&netstring::encode(b"main"));
        wire.extend_from_slice(&netstring::encode(b""));
        let result = Message::from_wire(&wire);
        assert!(matches!(result, Err(ProtocolError::InvalidField { .. })));
    }
}
