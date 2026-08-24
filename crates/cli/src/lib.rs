//! Library surface of the Lazarus CLI. The binary in `main.rs` is a thin
//! shell over this crate so integration tests (and the Desktop, which
//! delegates every lifecycle mutation here) drive exactly the code users
//! run.

pub mod client;
pub mod host;
pub mod updater;
