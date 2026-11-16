//! Pinned Bot API wire model and metadata.

pub mod catalog;
#[rustfmt::skip]
pub mod types;
#[cfg(test)]
#[rustfmt::skip]
mod fixtures;

pub use crate::wire::{ChatTarget, True, UnknownVariant, Zero};
