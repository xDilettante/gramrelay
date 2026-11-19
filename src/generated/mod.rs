//! Pinned Bot API wire model and metadata.

pub mod catalog;
#[rustfmt::skip]
pub mod types;
#[rustfmt::skip]
pub mod methods;
#[cfg(test)]
#[rustfmt::skip]
mod fixtures;
#[cfg(test)]
#[rustfmt::skip]
mod method_fixtures;

pub use crate::typed_api::{Api, Call, Effect, Method, ReplaySafety};
pub use crate::wire::{ChatTarget, SecretString, True, UnknownVariant, Zero};
