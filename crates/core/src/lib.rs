//! Core library for the capsule prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

pub mod acquire;
pub mod git;
pub mod init;
pub mod plan;
pub mod render;
pub mod view;

#[cfg(test)]
pub(crate) mod test_utils;
