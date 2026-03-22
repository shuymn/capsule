//! Wire protocol for the capsule prompt engine.
//!
//! Provides netstring encoding/decoding, typed messages (`Request`, `RenderResult`, `Update`),
//! and async codec for reading/writing messages over byte streams.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

pub mod codec;
pub mod generation;
pub mod message;
pub mod netstring;

pub use codec::{MessageReader, MessageWriter};
pub use generation::{ConfigGeneration, DepHash, PromptGeneration};
pub use message::{
    BuildId, Hello, HelloAck, Message, PROTOCOL_VERSION, RenderResult, Request, SessionId,
    StatusRequest, StatusResponse, Update,
};

/// Errors that can occur during protocol operations.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Netstring length prefix is not a valid number or overflows.
    #[error("netstring: invalid length prefix")]
    InvalidLength,

    /// Netstring is missing the `:` separator after length.
    #[error("netstring: missing colon separator")]
    MissingColon,

    /// Netstring is missing the trailing `,`.
    #[error("netstring: missing trailing comma")]
    MissingComma,

    /// Input ends before the netstring data and trailing comma.
    #[error("netstring: input truncated")]
    Truncated,

    /// The message type discriminator is not recognized.
    #[error("message: unknown type")]
    UnknownMessageType,

    /// The message has the wrong number of netstring fields.
    #[error("message: expected {expected} fields, got {got}")]
    WrongFieldCount {
        /// Expected field count.
        expected: usize,
        /// Actual field count.
        got: usize,
    },

    /// A message field contains an invalid value.
    #[error("message: invalid `{field}`: {reason}")]
    InvalidField {
        /// Which field failed.
        field: &'static str,
        /// Why it failed.
        reason: &'static str,
    },

    /// An I/O error occurred during codec read/write.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
