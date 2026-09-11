//! XOS command line client.
//!
//! A thin client over xosd. It holds no intelligence of its own: every command
//! is a JSON-RPC call over the daemon's Unix socket.

mod socket;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use socket::Connection;

#[derive(Parser, Debug)]
#[command(name = "xos", about = "Talk to the XOS daemon", version)]
struct Args {
    /// Path to the daemon socket. Overrides the config and the defaults.
    #[arg(long, global = true, env = "XOS_SOCKET")]
    socket: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Stop every autonomous action at once.
    Halt,
    /// Clear a halt and let autonomous work run again.
    Resume,
    /// Report daemon health, the halt flag and the configured providers.
    Status,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();

    let mut connection = match Connection::open(args.socket.clone()) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("xos: {}", error);
            eprintln!("     Start it with `xosd`, or point at it with --socket.");
            return std::process::ExitCode::FAILURE;
        }
    };

    let outcome = match args.command {
        Command::Halt => halt(&mut connection),
        Command::Resume => resume(&mut connection),
        Command::Status => status(&mut connection),
    }
    .and_then(|text| emit(&text));

    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xos: {}", error);
            std::process::ExitCode::FAILURE
        }
    }
}

/// Write output, treating a closed pipe as an ordinary end rather than a fault.
/// `xos status | head` is a normal thing to type.
fn emit(text: &str) -> Result<(), String> {
    use std::io::{self, Write};
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    match handle.write_all(text.as_bytes()).and_then(|_| handle.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn halt(connection: &mut Connection) -> Result<String, String> {
    connection.call("halt", json!({}))?;
    Ok("Halted. Every autonomous action is stopped, and stays stopped across a restart.
Run `xos resume` to start it again.
".to_string())
}

fn resume(connection: &mut Connection) -> Result<String, String> {
    connection.call("resume", json!({}))?;
    Ok("Resumed. Autonomous work can run again.
".to_string())
}

fn status(connection: &mut Connection) -> Result<String, String> {
    let result = connection.call("status", json!({}))?;

    let halted = result
        .get("halted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let version = result
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let default_provider = result
        .get("default_provider")
        .and_then(Value::as_str)
        .unwrap_or("none");

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "xosd      {}", version);
    let _ = writeln!(out, "socket    {}", connection.path().display());
    let _ = writeln!(
        out,
        "state     {}",
        if halted { "halted" } else { "running" }
    );
    let _ = writeln!(out, "default   {}", default_provider);

    let providers = result
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let _ = writeln!(out);
    if providers.is_empty() {
        let _ = writeln!(
            out,
            "No providers are configured. Add one to ~/.config/xos/config.toml."
        );
        return Ok(out);
    }

    let _ = writeln!(
        out,
        "{:<14} {:<8} {:>9} {:>7} {:>7}",
        "provider", "where", "context", "tools", "cache"
    );
    for provider in providers {
        let name = provider.get("name").and_then(Value::as_str).unwrap_or("-");
        let capabilities = provider.get("capabilities").cloned().unwrap_or(Value::Null);
        let local = capabilities
            .get("local")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let context = capabilities
            .get("context_window")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let tools = capabilities
            .get("supports_tools")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let cache = capabilities
            .get("supports_prefix_cache")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _ = writeln!(
            out,
            "{:<14} {:<8} {:>9} {:>7} {:>7}",
            name,
            if local { "local" } else { "api" },
            context,
            yes_no(tools),
            yes_no(cache)
        );
    }

    if halted {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Autonomous work is stopped. Run `xos resume` to start it again."
        );
    }
    Ok(out)
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
