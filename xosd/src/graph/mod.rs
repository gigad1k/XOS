//! XOS Goals — long-running goal decomposition and execution.
//!
//! Stores goals as a task graph, runs nodes on the local model, and reports
//! node-level progress so slow agent work reads as working rather than hung.
