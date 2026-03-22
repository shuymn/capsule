//! Newtypes for numeric values that must not be confused on the wire or in the daemon.
//!
//! Use [`PromptGeneration::from_wire`] / [`ConfigGeneration::from_wire`] when constructing
//! values from decoded netstring fields so validation stays at the parse boundary.

use crate::ProtocolError;

/// Monotonic prompt request generation from the shell client (`Request` / `RenderResult` / `Update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PromptGeneration(u64);

/// Configuration reload generation (daemon status and cache keying).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ConfigGeneration(u64);

/// Hash of slow-module dependency inputs for cache lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct DepHash(u64);

impl PromptGeneration {
    /// Wrap a raw wire value (tests and in-crate message builders).
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Construct from a decoded `generation` netstring field.
    ///
    /// Wire format allows any `u64` (including `0`); tighten here if the protocol evolves.
    ///
    /// # Errors
    ///
    /// Does not fail today; returns [`ProtocolError`] only after stricter validation is added.
    pub const fn from_wire(raw: u64) -> Result<Self, ProtocolError> {
        Ok(Self::new(raw))
    }

    /// Raw value for encoding or logging.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for PromptGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ConfigGeneration {
    /// Wrap a raw counter value (in-memory and tests).
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Construct from a decoded `config_generation` status field.
    ///
    /// # Errors
    ///
    /// Does not fail today; returns [`ProtocolError`] only after stricter validation is added.
    pub const fn from_wire(raw: u64) -> Result<Self, ProtocolError> {
        Ok(Self::new(raw))
    }

    /// Raw value for encoding or logging.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ConfigGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl DepHash {
    /// Wrap a dependency hash from slow-path module inputs.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Raw hash bits.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_generation_from_wire_accepts_zero() -> Result<(), ProtocolError> {
        assert_eq!(PromptGeneration::from_wire(0)?, PromptGeneration::new(0));
        Ok(())
    }

    #[test]
    fn config_generation_from_wire_round_trip() -> Result<(), ProtocolError> {
        assert_eq!(ConfigGeneration::from_wire(7)?, ConfigGeneration::new(7));
        Ok(())
    }
}
