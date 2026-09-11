//! Routing between the local reflex model and the cloud supervisor.
//!
//! Decides which tier serves a request, escalates when the local model cannot
//! carry it, and records every local-to-API escalation with its trigger.
