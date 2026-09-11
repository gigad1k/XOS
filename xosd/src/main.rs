//! XOS Core — the intelligence layer daemon.
//!
//! Every other component is a thin client over this daemon's API. Nothing else
//! imports a model provider, MCP server or channel gateway directly.

mod graph;
mod memory;
mod policy;
mod providers;
mod router;
mod scheduler;
mod state;
mod supervisor;
mod vault;

fn main() {
    println!("xosd {} — no subsystems started. See BUILD.md for what lands next.", env!("CARGO_PKG_VERSION"));
}
