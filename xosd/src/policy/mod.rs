//! Policy engine and egress protection.
//!
//! Classifies actions as reversible or irreversible, blocks reads of
//! designated-secret paths, and scans and redacts context bound for any API so
//! secrets never leave the machine.
