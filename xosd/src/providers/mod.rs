//! XOS Connect — connectors to model providers, MCP servers and channel gateways.
//!
//! The only place in XOS that knows a provider exists. Everything else reaches
//! inference, tools and channels through the daemon, never by importing a
//! provider directly.
