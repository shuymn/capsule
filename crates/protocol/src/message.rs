//! Typed messages for the capsule wire protocol.
//!
//! The wire format is a sequence of netstring-encoded fields terminated by `\n`.
//! Each message type has a fixed field order and a type discriminator.

use crate::{ProtocolError, netstring};

/// Protocol version for v1.
pub const PROTOCOL_VERSION: u8 = 1;

/// Wire type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageType {
    Request,
    RenderResult,
    Update,
    Hello,
    HelloAck,
    StatusRequest,
    StatusResponse,
}

impl MessageType {
    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Request => b"Q",
            Self::RenderResult => b"R",
            Self::Update => b"U",
            Self::Hello => b"H",
            Self::HelloAck => b"A",
            Self::StatusRequest => b"S",
            Self::StatusResponse => b"T",
        }
    }

    pub(crate) fn from_bytes(b: &[u8]) -> Option<Self> {
        match b {
            b"Q" => Some(Self::Request),
            b"R" => Some(Self::RenderResult),
            b"U" => Some(Self::Update),
            b"H" => Some(Self::Hello),
            b"A" => Some(Self::HelloAck),
            b"S" => Some(Self::StatusRequest),
            b"T" => Some(Self::StatusResponse),
            _ => None,
        }
    }
}

/// Binary fingerprint in `"size:mtime_nanos"` format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildId(String);

impl BuildId {
    /// Create a `BuildId` from a fingerprint string.
    #[must_use]
    pub const fn new(s: String) -> Self {
        Self(s)
    }

    /// Return the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

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
    /// Environment variables propagated from the shell (e.g. PATH).
    ///
    /// Wire format: `KEY=VALUE\0KEY=VALUE\0...` in the meta field (field\[9\]).
    /// Empty when the client does not send env vars (backward compatible).
    pub env_vars: Vec<(String, String)>,
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
    /// Binary fingerprint of the sender. `None` = cannot compute, skip negotiation.
    pub build_id: Option<BuildId>,
}

/// Build ID handshake: daemon → client.
///
/// Wire type: `A` (4 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloAck {
    /// Protocol version.
    pub version: u8,
    /// Binary fingerprint of the daemon. `None` = cannot compute.
    pub build_id: Option<BuildId>,
    /// Environment variable names the daemon needs from the shell.
    ///
    /// The client should include these in subsequent [`Request::env_vars`].
    /// Empty means no extra env vars are needed (backward compatible).
    pub env_var_names: Vec<String>,
}

/// Status request: client → daemon.
///
/// Wire type: `S` (2 fields: version, type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRequest {
    pub version: u8,
}

/// Status response: daemon → client.
///
/// Wire type: `T` (24 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StatusResponse {
    pub version: u8,
    pub pid: u32,
    pub uptime_secs: u64,
    // Cache
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub cache_ttl_expirations: u64,
    pub cache_entries: u64,
    pub inflight_coalesces: u64,
    // Request
    pub requests_total: u64,
    pub stale_discards: u64,
    // Slow compute
    pub slow_computes_started: u64,
    pub slow_compute_duration_us: u64,
    pub git_timeouts: u64,
    pub custom_module_timeouts: u64,
    // Session
    pub active_sessions: u64,
    pub sessions_pruned: u64,
    // Connection
    pub connections_total: u64,
    pub connections_active: u64,
    // Config
    pub config_generation: u64,
    pub config_reloads: u64,
    pub config_reload_errors: u64,
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
    /// A status request.
    StatusRequest(StatusRequest),
    /// A status response.
    StatusResponse(StatusResponse),
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Write the common header fields (version, type, `session_id`, generation) into `buf`.
fn encode_header(
    buf: &mut Vec<u8>,
    version: u8,
    type_tag: MessageType,
    sid: SessionId,
    generation: u64,
) {
    netstring::encode_into(buf, version.to_string().as_bytes());
    netstring::encode_into(buf, type_tag.as_bytes());
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
            MessageType::Request,
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
        let meta = encode_env_vars(&self.env_vars);
        netstring::encode_into(&mut buf, &meta);
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
            MessageType::RenderResult,
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
            MessageType::Update,
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

/// Encode a Hello/HelloAck message (version + type + optional build id).
fn encode_hello_wire(version: u8, type_tag: MessageType, build_id: Option<&BuildId>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    netstring::encode_into(&mut buf, version.to_string().as_bytes());
    netstring::encode_into(&mut buf, type_tag.as_bytes());
    let id_bytes = build_id.map_or("", BuildId::as_str);
    netstring::encode_into(&mut buf, id_bytes.as_bytes());
    buf
}

impl Hello {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        encode_hello_wire(self.version, MessageType::Hello, self.build_id.as_ref())
    }
}

impl HelloAck {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        netstring::encode_into(&mut buf, self.version.to_string().as_bytes());
        netstring::encode_into(&mut buf, MessageType::HelloAck.as_bytes());
        let id_bytes = self.build_id.as_ref().map_or("", BuildId::as_str);
        netstring::encode_into(&mut buf, id_bytes.as_bytes());
        // env_var_names: comma-separated list (empty string = no extra vars)
        let names = self.env_var_names.join(",");
        netstring::encode_into(&mut buf, names.as_bytes());
        buf
    }
}

impl StatusRequest {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16);
        netstring::encode_into(&mut buf, self.version.to_string().as_bytes());
        netstring::encode_into(&mut buf, MessageType::StatusRequest.as_bytes());
        buf
    }
}

impl StatusResponse {
    const FIELD_COUNT: usize = 24;

    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);
        netstring::encode_into(&mut buf, self.version.to_string().as_bytes());
        netstring::encode_into(&mut buf, MessageType::StatusResponse.as_bytes());
        for val in [
            u64::from(self.pid),
            self.uptime_secs,
            self.cache_hits,
            self.cache_misses,
            self.cache_evictions,
            self.cache_ttl_expirations,
            self.cache_entries,
            self.inflight_coalesces,
            self.requests_total,
            self.stale_discards,
            self.slow_computes_started,
            self.slow_compute_duration_us,
            self.git_timeouts,
            self.custom_module_timeouts,
            self.active_sessions,
            self.sessions_pruned,
            self.connections_total,
            self.connections_active,
            self.config_generation,
            self.config_reloads,
            self.config_reload_errors,
        ] {
            netstring::encode_into(&mut buf, val.to_string().as_bytes());
        }
        // reserved field
        netstring::encode_into(&mut buf, b"");
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
            Self::StatusRequest(s) => s.to_wire(),
            Self::StatusResponse(s) => s.to_wire(),
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

        match MessageType::from_bytes(fields[1]) {
            Some(MessageType::Request) => {
                Ok(Self::Request(Request::from_fields(version, &fields)?))
            }
            Some(MessageType::RenderResult) => Ok(Self::RenderResult(RenderResult::from_fields(
                version, &fields,
            )?)),
            Some(MessageType::Update) => Ok(Self::Update(Update::from_fields(version, &fields)?)),
            Some(MessageType::Hello) => Ok(Self::Hello(Hello::from_fields(version, &fields)?)),
            Some(MessageType::HelloAck) => {
                Ok(Self::HelloAck(HelloAck::from_fields(version, &fields)?))
            }
            Some(MessageType::StatusRequest) => Ok(Self::StatusRequest(StatusRequest { version })),
            Some(MessageType::StatusResponse) => Ok(Self::StatusResponse(
                StatusResponse::from_fields(version, &fields)?,
            )),
            None => Err(ProtocolError::UnknownMessageType),
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

/// Encode env vars as `KEY=VALUE\0KEY=VALUE\0...` bytes.
fn encode_env_vars(vars: &[(String, String)]) -> Vec<u8> {
    if vars.is_empty() {
        return Vec::new();
    }
    let cap: usize = vars
        .iter()
        .map(|(k, v)| k.len() + 1 + v.len())
        .sum::<usize>()
        + vars.len().saturating_sub(1);
    let mut buf = Vec::with_capacity(cap);
    for (i, (key, value)) in vars.iter().enumerate() {
        if i > 0 {
            buf.push(0); // null separator
        }
        buf.extend_from_slice(key.as_bytes());
        buf.push(b'=');
        buf.extend_from_slice(value.as_bytes());
    }
    buf
}

/// Decode env vars from `KEY=VALUE\0KEY=VALUE\0...` bytes.
/// Empty input returns an empty vec (backward compatible with old clients).
fn decode_env_vars(field: &[u8]) -> Vec<(String, String)> {
    if field.is_empty() {
        return Vec::new();
    }
    let mut vars = Vec::with_capacity(4);
    for part in field.split(|&b| b == 0) {
        if let Some(eq_pos) = part.iter().position(|&b| b == b'=')
            && let (Ok(key), Ok(value)) = (
                std::str::from_utf8(&part[..eq_pos]),
                std::str::from_utf8(&part[eq_pos + 1..]),
            )
        {
            vars.push((key.to_owned(), value.to_owned()));
        }
    }
    vars
}

/// Parse a comma-separated list of names. Empty input returns empty vec.
fn parse_comma_list(field: &[u8]) -> Vec<String> {
    let Ok(s) = std::str::from_utf8(field) else {
        return vec![];
    };
    if s.is_empty() {
        return vec![];
    }
    s.split(',').map(ToOwned::to_owned).collect()
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
            env_vars: decode_env_vars(fields[9]),
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

/// Parse an optional `BuildId` from a wire field (empty = `None`).
fn parse_opt_build_id(field: &[u8]) -> Result<Option<BuildId>, ProtocolError> {
    if field.is_empty() {
        Ok(None)
    } else {
        Ok(Some(BuildId::new(field_to_string(field, "build_id")?)))
    }
}

/// Validate field count for a Hello/HelloAck message (3 fields).
const HELLO_FIELD_COUNT: usize = 3;

impl Hello {
    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != HELLO_FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: HELLO_FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            build_id: parse_opt_build_id(fields[2])?,
        })
    }
}

const HELLO_ACK_FIELD_COUNT: usize = 4;

impl HelloAck {
    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != HELLO_ACK_FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: HELLO_ACK_FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            build_id: parse_opt_build_id(fields[2])?,
            env_var_names: parse_comma_list(fields[3]),
        })
    }
}

impl StatusResponse {
    fn from_fields(version: u8, fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            version,
            pid: parse_field::<u32>(fields[2], "pid")?,
            uptime_secs: parse_field(fields[3], "uptime_secs")?,
            cache_hits: parse_field(fields[4], "cache_hits")?,
            cache_misses: parse_field(fields[5], "cache_misses")?,
            cache_evictions: parse_field(fields[6], "cache_evictions")?,
            cache_ttl_expirations: parse_field(fields[7], "cache_ttl_expirations")?,
            cache_entries: parse_field(fields[8], "cache_entries")?,
            inflight_coalesces: parse_field(fields[9], "inflight_coalesces")?,
            requests_total: parse_field(fields[10], "requests_total")?,
            stale_discards: parse_field(fields[11], "stale_discards")?,
            slow_computes_started: parse_field(fields[12], "slow_computes_started")?,
            slow_compute_duration_us: parse_field(fields[13], "slow_compute_duration_us")?,
            git_timeouts: parse_field(fields[14], "git_timeouts")?,
            custom_module_timeouts: parse_field(fields[15], "custom_module_timeouts")?,
            active_sessions: parse_field(fields[16], "active_sessions")?,
            sessions_pruned: parse_field(fields[17], "sessions_pruned")?,
            connections_total: parse_field(fields[18], "connections_total")?,
            connections_active: parse_field(fields[19], "connections_active")?,
            config_generation: parse_field(fields[20], "config_generation")?,
            config_reloads: parse_field(fields[21], "config_reloads")?,
            config_reload_errors: parse_field(fields[22], "config_reload_errors")?,
            // fields[23] = reserved
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
            env_vars: vec![],
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
            build_id: Some(BuildId::new("12345:1700000000000000000".to_owned())),
        }
    }

    fn sample_hello_ack() -> HelloAck {
        HelloAck {
            version: PROTOCOL_VERSION,
            build_id: Some(BuildId::new("12345:1700000000000000000".to_owned())),
            env_var_names: vec![],
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

    #[test]
    fn test_request_with_env_vars() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned())];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_with_multiple_env_vars() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned()),
            ("HOME".to_owned(), "/home/user".to_owned()),
        ];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_empty_env_vars_backward_compat() -> Result<(), ProtocolError> {
        let req = sample_request();
        assert!(req.env_vars.is_empty());
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
    fn test_hello_none_build_id() -> Result<(), ProtocolError> {
        let hello = Hello {
            version: PROTOCOL_VERSION,
            build_id: None,
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

    #[test]
    fn test_hello_ack_with_env_var_names_round_trip() -> Result<(), ProtocolError> {
        let ack = HelloAck {
            version: PROTOCOL_VERSION,
            build_id: Some(BuildId::new("test:123".to_owned())),
            env_var_names: vec!["AWS_PROFILE".to_owned(), "TERRAFORM_WORKSPACE".to_owned()],
        };
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

    // -- Env var edge cases --

    #[test]
    fn test_request_env_vars_empty_key() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![(String::new(), "value".to_owned())];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_empty_value() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![("PATH".to_owned(), String::new())];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_empty_key_and_value() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![(String::new(), String::new())];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_value_contains_equals() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![("PATH".to_owned(), "/usr/bin:dir=with=equals".to_owned())];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_large_value() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![("PATH".to_owned(), "/usr/local/bin:".repeat(500))];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_shell_metacharacters() -> Result<(), ProtocolError> {
        let mut req = sample_request();
        req.env_vars = vec![(
            "PATH".to_owned(),
            "/usr/bin;rm -rf /:$(evil):`evil`:$((1+1))".to_owned(),
        )];
        let wire = req.to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::Request(req));
        Ok(())
    }

    // -- Env var wire-level edge cases (hand-crafted bytes) --

    /// Build a 10-field Q message from raw bytes with a custom meta field,
    /// then decode it and return the `env_vars`.
    fn decode_env_vars_from_wire(meta: &[u8]) -> Result<Vec<(String, String)>, ProtocolError> {
        let mut wire = Vec::new();
        netstring::encode_into(&mut wire, b"1");
        netstring::encode_into(&mut wire, b"Q");
        netstring::encode_into(&mut wire, b"0123456789abcdef");
        netstring::encode_into(&mut wire, b"1");
        netstring::encode_into(&mut wire, b"/tmp");
        netstring::encode_into(&mut wire, b"80");
        netstring::encode_into(&mut wire, b"0");
        netstring::encode_into(&mut wire, b"");
        netstring::encode_into(&mut wire, b"main");
        netstring::encode_into(&mut wire, meta);
        let Message::Request(req) = Message::from_wire(&wire)? else {
            return Ok(vec![]);
        };
        Ok(req.env_vars)
    }

    /// Null byte in the wire meta field splits into separate entries.
    /// Rust `String` cannot contain null bytes, so the encoder never produces
    /// this — the decoder handles it consistently by treating null as separator.
    #[test]
    fn test_request_env_vars_null_byte_in_wire() -> Result<(), ProtocolError> {
        let vars = decode_env_vars_from_wire(b"PATH=/usr/bin\0INJECT=evil")?;
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], ("PATH".to_owned(), "/usr/bin".to_owned()));
        assert_eq!(vars[1], ("INJECT".to_owned(), "evil".to_owned()));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_non_utf8_in_wire() -> Result<(), ProtocolError> {
        let vars = decode_env_vars_from_wire(b"PATH=\xff\xfe/usr/bin")?;
        assert!(vars.is_empty(), "non-UTF-8 entries should be dropped");
        Ok(())
    }

    #[test]
    fn test_request_env_vars_old_client_empty_meta() -> Result<(), ProtocolError> {
        let vars = decode_env_vars_from_wire(b"")?;
        assert!(
            vars.is_empty(),
            "empty meta must decode to empty env_vars for backward compat"
        );
        Ok(())
    }

    #[test]
    fn test_request_env_vars_bare_equals_in_wire() -> Result<(), ProtocolError> {
        let vars = decode_env_vars_from_wire(b"=")?;
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0], (String::new(), String::new()));
        Ok(())
    }

    #[test]
    fn test_request_env_vars_no_equals_in_wire() -> Result<(), ProtocolError> {
        let vars = decode_env_vars_from_wire(b"MALFORMED_NO_EQUALS")?;
        assert!(vars.is_empty(), "entry without '=' should be dropped");
        Ok(())
    }

    #[test]
    fn test_status_request_round_trip() -> Result<(), ProtocolError> {
        let req = StatusRequest {
            version: PROTOCOL_VERSION,
        };
        let wire = Message::StatusRequest(req).to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(
            parsed,
            Message::StatusRequest(StatusRequest {
                version: PROTOCOL_VERSION,
            })
        );
        Ok(())
    }

    #[test]
    fn test_status_response_round_trip() -> Result<(), ProtocolError> {
        let resp = StatusResponse {
            version: PROTOCOL_VERSION,
            pid: 12345,
            uptime_secs: 3600,
            cache_hits: 100,
            cache_misses: 10,
            cache_evictions: 2,
            cache_ttl_expirations: 1,
            cache_entries: 42,
            inflight_coalesces: 5,
            requests_total: 110,
            stale_discards: 3,
            slow_computes_started: 10,
            slow_compute_duration_us: 500_000,
            git_timeouts: 1,
            custom_module_timeouts: 0,
            active_sessions: 3,
            sessions_pruned: 7,
            connections_total: 50,
            connections_active: 2,
            config_generation: 1,
            config_reloads: 1,
            config_reload_errors: 0,
        };
        let wire = Message::StatusResponse(resp.clone()).to_wire();
        let parsed = Message::from_wire(&wire)?;
        assert_eq!(parsed, Message::StatusResponse(resp));
        Ok(())
    }
}
