//! Typed messages for the capsule wire protocol.
//!
//! The wire format is a sequence of netstring-encoded fields terminated by `\n`.
//! Each message type has a fixed field order and a type discriminator.

use crate::{
    ProtocolError,
    generation::{ConfigGeneration, PromptGeneration},
    netstring,
};

/// Protocol version for v1.
pub const PROTOCOL_VERSION: u8 = 1;

/// Pre-encoded ASCII form of [`PROTOCOL_VERSION`] for wire emission without per-call allocation.
const PROTOCOL_VERSION_BYTES: &[u8] = b"1";
const _: () = assert!(
    PROTOCOL_VERSION == 1,
    "PROTOCOL_VERSION_BYTES is out of sync"
);

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
    /// Session identifier.
    pub session_id: SessionId,
    /// Monotonically increasing generation counter.
    pub generation: PromptGeneration,
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
    /// Session identifier.
    pub session_id: SessionId,
    /// Generation this response corresponds to.
    pub generation: PromptGeneration,
    /// Info line (line 1 of the prompt).
    pub left1: String,
    /// Input line (line 2 of the prompt).
    pub left2: String,
    /// Opaque metadata for shell-side features (e.g., vim mode character map).
    ///
    /// Empty when no metadata is needed (backward compatible).
    pub meta: String,
}

/// Deferred update after slow modules complete.
///
/// Wire type: `U` (8 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// Session identifier.
    pub session_id: SessionId,
    /// Generation this update corresponds to.
    pub generation: PromptGeneration,
    /// Updated info line.
    pub left1: String,
    /// Updated input line.
    pub left2: String,
    /// Opaque metadata for shell-side features (e.g., vim mode character map).
    ///
    /// Empty when no metadata is needed (backward compatible).
    pub meta: String,
}

/// Build ID handshake: client → daemon.
///
/// Wire type: `H` (3 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Binary fingerprint of the sender. `None` = cannot compute, skip negotiation.
    pub build_id: Option<BuildId>,
}

/// Build ID handshake: daemon → client.
///
/// Wire type: `A` (4 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloAck {
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
pub struct StatusRequest;

/// Status response: daemon → client.
///
/// Wire type: `T` (23 fields).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StatusResponse {
    pub pid: u32,
    pub uptime_secs: u64,
    // Cache
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
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
    pub config_generation: ConfigGeneration,
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
    type_tag: MessageType,
    sid: SessionId,
    generation: PromptGeneration,
) {
    netstring::encode_into(buf, PROTOCOL_VERSION_BYTES);
    netstring::encode_into(buf, type_tag.as_bytes());
    netstring::encode_into(buf, sid.to_string().as_bytes());
    netstring::encode_into(buf, generation.get().to_string().as_bytes());
}

impl Request {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        encode_header(
            &mut buf,
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
            MessageType::RenderResult,
            self.session_id,
            self.generation,
        );
        netstring::encode_into(&mut buf, self.left1.as_bytes());
        netstring::encode_into(&mut buf, self.left2.as_bytes());
        netstring::encode_into(&mut buf, b""); // right1 (reserved)
        netstring::encode_into(&mut buf, self.meta.as_bytes());
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
            MessageType::Update,
            self.session_id,
            self.generation,
        );
        netstring::encode_into(&mut buf, self.left1.as_bytes());
        netstring::encode_into(&mut buf, self.left2.as_bytes());
        netstring::encode_into(&mut buf, b""); // right1 (reserved)
        netstring::encode_into(&mut buf, self.meta.as_bytes());
        buf
    }
}

/// Encode a Hello/HelloAck message (version + type + optional build id).
fn encode_hello_wire(type_tag: MessageType, build_id: Option<&BuildId>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    netstring::encode_into(&mut buf, PROTOCOL_VERSION_BYTES);
    netstring::encode_into(&mut buf, type_tag.as_bytes());
    let id_bytes = build_id.map_or("", BuildId::as_str);
    netstring::encode_into(&mut buf, id_bytes.as_bytes());
    buf
}

impl Hello {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        encode_hello_wire(MessageType::Hello, self.build_id.as_ref())
    }
}

impl HelloAck {
    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = encode_hello_wire(MessageType::HelloAck, self.build_id.as_ref());
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
        netstring::encode_into(&mut buf, PROTOCOL_VERSION_BYTES);
        netstring::encode_into(&mut buf, MessageType::StatusRequest.as_bytes());
        buf
    }
}

impl StatusResponse {
    const FIELD_COUNT: usize = 23;

    /// Serialize to wire format (without trailing LF).
    #[must_use]
    pub fn to_wire(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);
        netstring::encode_into(&mut buf, PROTOCOL_VERSION_BYTES);
        netstring::encode_into(&mut buf, MessageType::StatusResponse.as_bytes());
        for val in [
            u64::from(self.pid),
            self.uptime_secs,
            self.cache_hits,
            self.cache_misses,
            self.cache_evictions,
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
            self.config_generation.get(),
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
            Some(MessageType::Request) => Ok(Self::Request(Request::from_fields(&fields)?)),
            Some(MessageType::RenderResult) => {
                Ok(Self::RenderResult(RenderResult::from_fields(&fields)?))
            }
            Some(MessageType::Update) => Ok(Self::Update(Update::from_fields(&fields)?)),
            Some(MessageType::Hello) => Ok(Self::Hello(Hello::from_fields(&fields)?)),
            Some(MessageType::HelloAck) => Ok(Self::HelloAck(HelloAck::from_fields(&fields)?)),
            Some(MessageType::StatusRequest) => {
                Ok(Self::StatusRequest(StatusRequest::from_fields(&fields)?))
            }
            Some(MessageType::StatusResponse) => {
                Ok(Self::StatusResponse(StatusResponse::from_fields(&fields)?))
            }
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

    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            session_id: SessionId::from_hex(fields[2])?,
            generation: PromptGeneration::from_wire(parse_field::<u64>(fields[3], "generation")?)?,
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

    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            session_id: SessionId::from_hex(fields[2])?,
            generation: PromptGeneration::from_wire(parse_field::<u64>(fields[3], "generation")?)?,
            left1: field_to_string(fields[4], "left1")?,
            left2: field_to_string(fields[5], "left2")?,
            // fields[6] = right1 (ignored)
            meta: field_to_string(fields[7], "meta")?,
        })
    }
}

impl Update {
    const FIELD_COUNT: usize = 8;

    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            session_id: SessionId::from_hex(fields[2])?,
            generation: PromptGeneration::from_wire(parse_field::<u64>(fields[3], "generation")?)?,
            left1: field_to_string(fields[4], "left1")?,
            left2: field_to_string(fields[5], "left2")?,
            // fields[6] = right1 (ignored)
            meta: field_to_string(fields[7], "meta")?,
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

impl Hello {
    const FIELD_COUNT: usize = 3;

    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            build_id: parse_opt_build_id(fields[2])?,
        })
    }
}

impl HelloAck {
    const FIELD_COUNT: usize = 4;

    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            build_id: parse_opt_build_id(fields[2])?,
            env_var_names: parse_comma_list(fields[3]),
        })
    }
}

impl StatusRequest {
    const FIELD_COUNT: usize = 2;

    const fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self)
    }
}

impl StatusResponse {
    fn from_fields(fields: &[&[u8]]) -> Result<Self, ProtocolError> {
        if fields.len() != Self::FIELD_COUNT {
            return Err(ProtocolError::WrongFieldCount {
                expected: Self::FIELD_COUNT,
                got: fields.len(),
            });
        }
        Ok(Self {
            pid: parse_field::<u32>(fields[2], "pid")?,
            uptime_secs: parse_field(fields[3], "uptime_secs")?,
            cache_hits: parse_field(fields[4], "cache_hits")?,
            cache_misses: parse_field(fields[5], "cache_misses")?,
            cache_evictions: parse_field(fields[6], "cache_evictions")?,
            cache_entries: parse_field(fields[7], "cache_entries")?,
            inflight_coalesces: parse_field(fields[8], "inflight_coalesces")?,
            requests_total: parse_field(fields[9], "requests_total")?,
            stale_discards: parse_field(fields[10], "stale_discards")?,
            slow_computes_started: parse_field(fields[11], "slow_computes_started")?,
            slow_compute_duration_us: parse_field(fields[12], "slow_compute_duration_us")?,
            git_timeouts: parse_field(fields[13], "git_timeouts")?,
            custom_module_timeouts: parse_field(fields[14], "custom_module_timeouts")?,
            active_sessions: parse_field(fields[15], "active_sessions")?,
            sessions_pruned: parse_field(fields[16], "sessions_pruned")?,
            connections_total: parse_field(fields[17], "connections_total")?,
            connections_active: parse_field(fields[18], "connections_active")?,
            config_generation: ConfigGeneration::from_wire(parse_field(
                fields[19],
                "config_generation",
            )?)?,
            config_reloads: parse_field(fields[20], "config_reloads")?,
            config_reload_errors: parse_field(fields[21], "config_reload_errors")?,
            // fields[22] = reserved
        })
    }
}

#[cfg(test)]
mod tests;
