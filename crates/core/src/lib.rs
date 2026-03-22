//! Core library for the capsule prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod sealed;

pub mod config;
pub mod daemon;
pub mod init;
pub mod module;
pub mod render;

#[cfg(test)]
pub(crate) mod test_utils;
