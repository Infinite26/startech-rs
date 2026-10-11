//! net: Minecraft Java Edition protocol 763 (1.20.1) — offline mode.
//! See docs/PROTOCOL-763.md for the authoritative packet reference.

pub mod connection;
pub mod packets;
pub mod types;

pub use connection::{handle_client, serve_startup};
