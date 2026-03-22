//! Crate-private supertrait for [`crate::module::Module`] and [`crate::module::GitProvider`].
//!
//! Only types defined in `capsule-core` implement [`Sealed`], so downstream crates cannot add
//! new module or git provider implementations without a fork.

/// Marker implemented only inside this crate (see [`crate::module::Module`], [`crate::module::GitProvider`]).
pub trait Sealed {}
