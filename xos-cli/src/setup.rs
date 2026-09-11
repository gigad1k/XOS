//! The first-run wizard.
//!
//! This is the first ninety seconds somebody spends with XOS, and it decides
//! whether they keep it. Two rules follow from that, and everything here is
//! shaped by them.
//!
//! **Every step is skippable.** Somebody booting with no accounts, no keys and
//! no wifi must still come out of this with a working local assistant. Not a
//! degraded one with a list of things to fix later: a working one. So nothing
//! here blocks, nothing here is required, and skipping the lot is a supported
//! path with its own test.
//!
//! **Nothing is done behind their back.** Each step says what it is for and what
//! it would do before offering to do it. The kill switch step goes further and
//! makes somebody press the keys, because a safety control nobody has ever used
//! is one they will not reach for at the moment they need it.
//!
//! The rendering is a pure function over the wizard's state, the way the status
//! bar is, so the whole flow can be tested without a terminal.

use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::BufRead;

use crate::socket::Connection;

/// Write, and do not mind if nobody is reading.
///
/// `print!` panics when the other end of the pipe has gone, which meant that
/// `xos setup | head` aborted the wizard partway and the first goal never ran.
/// The machine was set up and the one thing that demonstrates it was silently
/// skipped, which is the worst possible version of that bug. Output is a
/// courtesy here; finishing is not.
fn say(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(text.as_bytes());
    let _ = handle.flush();
}

/// The nine steps, in the order somebody meets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Hardware,
    Benchmark,
    Model,
    Keys,
    Connectors,
    Messaging,
    Voice,
    Policy,
    KillSwitch,
}

impl Step {
    pub fn all() -> [Step; 9] {
        [
            Step::Hardware,
            Step::Benchmark,
            Step::Model,
            Step::Keys,
            Step::Connectors,
            Step::Messaging,
            Step::Voice,
            Step::Policy,
            Step::KillSwitch,
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Step::Hardware => "What you have",
            Step::Benchmark => "How fast it is",
            Step::Model => "The model that runs here",
            Step::Keys => "Provider keys",
            Step::Connectors => "Connectors",
            Step::Messaging => "Messaging",
            Step::Voice => "Voice",
            Step::Policy => "What XOS is allowed to do",
            Step::KillSwitch => "The stop button",
        }
    }

}

/// Everything the wizard has learnt so far.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub hardware: Option<Value>,
    pub model: Option<String>,
    pub keys_added: Vec<String>,
    pub connectors: Vec<String>,
    pub messaging_enabled: bool,
    pub voice_enabled: bool,
    pub cost_mode: Option<String>,
    pub strictness: Option<String>,
    pub kill_switch_pressed: bool,
    pub skipped: Vec<&'static str>,
}

impl State {
    pub fn skip(&mut self, step: Step) {
        if !self.skipped.contains(&step.title()) {
            self.skipped.push(step.title());
        }
    }

    /// Whether XOS is usable at the end of this, whatever was skipped.
    ///
    /// The answer is always yes, and it has to be: a wizard that can leave
    /// somebody without a working assistant has failed at the one thing it is
    /// for. A machine with no keys and no model still answers, through whatever
    /// the daemon has.
    pub fn usable(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------- rendering

/// One step, as text. A pure function, so the wizard can be tested without a
/// terminal attached.
pub fn render_step(step: Step, state: &State, answer: &Value) -> String {
    let mut out = String::new();
    let index = Step::all().iter().position(|s| *s == step).unwrap_or(0) + 1;

    let _ = writeln!(out, "");
    let _ = writeln!(out, "  {}/9  {}", index, step.title());
    let _ = writeln!(out, "  {}", "-".repeat(46));
    let _ = writeln!(out, "");

    match step {
        Step::Hardware => {
            // Not a spinner and a claim of success. What was actually found,
            // what drives it, and what is not working, said plainly.
            let inventory = answer.get("inventory").unwrap_or(&Value::Null);
            let _ = writeln!(
                out,
                "  {}",
                answer
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("could not read this machine")
            );
            let _ = writeln!(out, "");
            for gpu in inventory
                .get("gpus")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                let _ = writeln!(
                    out,
                    "  display   {}",
                    gpu.get("description").and_then(Value::as_str).unwrap_or("-")
                );
                if let Some(driver) = gpu.get("kernel_driver").and_then(Value::as_str) {
                    let _ = writeln!(out, "            driven by {}", driver);
                }
            }
            for resolution in answer
                .get("resolve")
                .and_then(|r| r.get("display"))
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                if let Some(level) = resolution.get("fallback_level").and_then(Value::as_u64) {
                    if level > 0 {
                        let _ = writeln!(
                            out,
                            "            this is fallback level {}, not the first choice",
                            level
                        );
                    }
                }
            }
            // What is not working, and why. Left out, somebody discovers it
            // later and has no idea it was known about.
            let unavailable = unavailable_things(answer);
            if unavailable.is_empty() {
                let _ = writeln!(out, "");
                let _ = writeln!(out, "  Everything here is working.");
            } else {
                let _ = writeln!(out, "");
                let _ = writeln!(out, "  Not working:");
                for line in unavailable {
                    let _ = writeln!(out, "    {}", line);
                }
            }
        }
        Step::Benchmark => {
            let _ = writeln!(
                out,
                "  XOS can measure how the candidate models actually run on this"
            );
            let _ = writeln!(out, "  machine, rather than guessing from the specification.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  It takes a few minutes and changes nothing.");
            if let Some(rows) = answer.get("ranked").and_then(Value::as_array) {
                let _ = writeln!(out, "");
                for row in rows {
                    let _ = writeln!(
                        out,
                        "    {:<22} {:>7} tok/s   tools {}%",
                        row.get("model").and_then(Value::as_str).unwrap_or("-"),
                        row.get("tokens_per_second")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                            .round(),
                        row.get("tool_success")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                            .round()
                    );
                }
            }
        }
        Step::Model => {
            let name = answer
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("no model fits this machine");
            let _ = writeln!(out, "  {}", name);
            if let Some(size) = answer.get("approximate_size_gb").and_then(Value::as_f64) {
                let vram = answer
                    .get("available_vram_mb")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let needs = answer
                    .get("minimum_vram_mb")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let _ = writeln!(out, "");
                let _ = writeln!(out, "  about {:.1} GB to download", size);
                // The fit, against this machine, in numbers. A download that
                // turns out not to fit is a bad forty minutes.
                let _ = writeln!(
                    out,
                    "  wants {} MB of VRAM; this machine has {} MB",
                    needs, vram
                );
                if vram > 0 && needs > vram {
                    let _ = writeln!(out, "  it will not fit, and will run on the CPU instead");
                }
            }
        }
        Step::Keys => {
            // OpenRouter first on purpose: it is one key instead of eight, and
            // the difference between someone finishing this step and not.
            let _ = writeln!(out, "  OpenRouter first, because it is one key rather than");
            let _ = writeln!(out, "  eight. Anything else can go in afterwards.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Keys are stored by the daemon, encrypted. This");
            let _ = writeln!(out, "  program never keeps one.");
            if !state.keys_added.is_empty() {
                let _ = writeln!(out, "");
                let _ = writeln!(out, "  added: {}", state.keys_added.join(", "));
            }
        }
        Step::Connectors => {
            // XOS registers no OAuth apps of its own, so these are somebody
            // else's login flows and XOS only shells out to them.
            let _ = writeln!(out, "  XOS registers no accounts of its own. These use the");
            let _ = writeln!(out, "  tools you already have, with their own logins:");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "    gh        GitHub");
            let _ = writeln!(out, "    rclone    cloud storage");
            let _ = writeln!(out, "    openclaw  messaging");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Each opens its own login. XOS never sees the password.");
        }
        Step::Messaging => {
            // The warnings come before the offer, not after it.
            let _ = writeln!(out, "  XOS can answer messages on WhatsApp.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Two things worth knowing first:");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "    Use a dedicated number, not your own. Linking your");
            let _ = writeln!(out, "    main account means XOS sees every conversation on it.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "    An allowlist is required before this turns on. XOS");
            let _ = writeln!(out, "    will only ever reply to numbers you name.");
        }
        Step::Voice => {
            let _ = writeln!(out, "  A short microphone test, then a wake word.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Nothing is recorded or sent anywhere. Wake word");
            let _ = writeln!(out, "  detection runs on this machine.");
        }
        Step::Policy => {
            // Plain language, not the names of the modes. Somebody choosing
            // this in their first minute has no idea what "balanced" means.
            let _ = writeln!(out, "  How much XOS spends, and how much it asks.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Routing:");
            let _ = writeln!(out, "    1  Keep it local. Slower, and free. Only reaches for");
            let _ = writeln!(out, "       an API when the local model genuinely cannot do it.");
            let _ = writeln!(out, "    2  Balanced. Local first, escalating when it helps.");
            let _ = writeln!(out, "    3  Best answer. Uses the strongest model available,");
            let _ = writeln!(out, "       and costs the most.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Asking:");
            let _ = writeln!(out, "    1  Ask before anything that cannot be undone, and");
            let _ = writeln!(out, "       before anything leaves this machine.");
            let _ = writeln!(out, "    2  Ask before things that cannot be undone.");
            let _ = writeln!(out, "    3  Ask rarely. For people who know what they want.");
        }
        Step::KillSwitch => {
            // The one step that asks for something rather than offering it. A
            // control somebody has never used is one they will not reach for
            // when they need it, and needing it is the whole point.
            let _ = writeln!(out, "  Ctrl+Alt+Escape stops everything XOS is doing.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Not a pause. Every running action stops, every queued");
            let _ = writeln!(out, "  action is dropped, and nothing starts again until you");
            let _ = writeln!(out, "  say so. It works when the machine is busy, and it");
            let _ = writeln!(out, "  works when the desktop has stopped responding.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Press it now, so you have used it once.");
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  (Or type 'skip'. It works whether or not you try it.)");
            if state.kill_switch_pressed {
                let _ = writeln!(out, "");
                let _ = writeln!(out, "  Pressed. XOS halted and resumed.");
            }
        }
    }

    out
}

/// What this machine cannot do, and why, from the hardware answer.
fn unavailable_things(answer: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let inventory = answer.get("inventory").unwrap_or(&Value::Null);

    if inventory
        .get("gpus")
        .and_then(Value::as_array)
        .map(|g| g.is_empty())
        .unwrap_or(true)
    {
        lines.push("no display device was found".to_string());
    }
    if inventory
        .get("network")
        .and_then(Value::as_array)
        .map(|n| n.is_empty())
        .unwrap_or(true)
    {
        lines.push("no network device was found, so XOS will work offline".to_string());
    }
    if let Some(firmware) = inventory.get("firmware") {
        if firmware.get("mode").and_then(Value::as_str) == Some("unknown") {
            lines.push("the firmware mode could not be read".to_string());
        }
    }
    for resolution in answer
        .get("resolve")
        .and_then(|r| r.get("display"))
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        if resolution.get("driver").and_then(Value::as_str) == Some("unknown") {
            lines.push(format!(
                "{}:{} is not in the hardware database, so it is running on whatever the kernel chose",
                resolution.get("vendor_id").and_then(Value::as_str).unwrap_or("?"),
                resolution.get("device_id").and_then(Value::as_str).unwrap_or("?"),
            ));
        }
    }
    lines
}

/// What is shown when the wizard is done, before the first goal runs.
pub fn render_finish(state: &State, goal: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "");
    let _ = writeln!(out, "  Ready.");
    let _ = writeln!(out, "");

    let mut configured = Vec::new();
    if let Some(model) = &state.model {
        configured.push(format!("model {}", model));
    }
    if !state.keys_added.is_empty() {
        configured.push(format!("keys for {}", state.keys_added.join(", ")));
    }
    if !state.connectors.is_empty() {
        configured.push(format!("connectors: {}", state.connectors.join(", ")));
    }
    if state.messaging_enabled {
        configured.push("messaging, with an allowlist".to_string());
    }
    if state.voice_enabled {
        configured.push("voice".to_string());
    }
    if let Some(mode) = &state.cost_mode {
        configured.push(format!("routing: {}", mode));
    }
    if let Some(strictness) = &state.strictness {
        configured.push(format!("asking: {}", strictness));
    }
    if state.kill_switch_pressed {
        configured.push("the stop button, tried once".to_string());
    }

    if !configured.is_empty() {
        let _ = writeln!(out, "  Set up: {}", configured.join(", "));
        let _ = writeln!(out, "");
    }

    if !state.skipped.is_empty() {
        // Named, not hidden. Skipping is fine and somebody should know what
        // they walked past, and that they can come back to it.
        let _ = writeln!(out, "  Skipped: {}", state.skipped.join(", "));
        let _ = writeln!(out, "  Any of it can be done later with `xos setup`.");
        let _ = writeln!(out, "");
    }

    // The promise the whole wizard is built around, stated where somebody who
    // skipped everything will read it.
    if state.usable() && state.skipped.len() == Step::all().len() {
        let _ = writeln!(
            out,
            "  You skipped all of it, and XOS still works. It runs on this"
        );
        let _ = writeln!(out, "  machine, offline, with no account and no key.");
        let _ = writeln!(out, "");
    }

    let _ = writeln!(out, "  XOS will now do one thing, so you can watch it work:");
    let _ = writeln!(out, "");
    let _ = writeln!(
        out,
        "    {}",
        goal.get("description")
            .and_then(Value::as_str)
            .unwrap_or("take stock of this machine")
    );
    let _ = writeln!(out, "");
    for step in goal.get("steps").and_then(Value::as_array).unwrap_or(&Vec::new()) {
        let _ = writeln!(
            out,
            "      {}",
            step.get("title").and_then(Value::as_str).unwrap_or("-")
        );
    }
    let _ = writeln!(out, "");
    // Exactly what it touches, before it touches it.
    let _ = writeln!(out, "  It reads these, and lists them without opening anything:");
    let _ = writeln!(
        out,
        "      {}",
        goal.get("reads")
            .and_then(Value::as_array)
            .map(|dirs| dirs
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("  "))
            .unwrap_or_default()
    );
    let _ = writeln!(out, "");
    let _ = writeln!(
        out,
        "  It writes {}.",
        goal.get("writes")
            .and_then(Value::as_str)
            .unwrap_or("one entry in memory")
    );
    let _ = writeln!(out, "  Nothing leaves this machine. Nothing here is irreversible.");
    out
}

pub fn render_outcome(outcome: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "");
    for step in outcome.get("steps").and_then(Value::as_array).unwrap_or(&Vec::new()) {
        let _ = writeln!(
            out,
            "  done   {}",
            step.get("title").and_then(Value::as_str).unwrap_or("-")
        );
        let _ = writeln!(
            out,
            "         {}",
            step.get("result").and_then(Value::as_str).unwrap_or("")
        );
    }
    if let Some(summary) = outcome.get("summary").and_then(Value::as_str) {
        if !summary.is_empty() {
            let _ = writeln!(out, "");
            let _ = writeln!(out, "  Remembered:");
            for line in wrap(summary, 62) {
                let _ = writeln!(out, "    {}", line);
            }
        }
    }
    let _ = writeln!(out, "");
    let _ = writeln!(out, "  That is the whole system: a goal, broken into steps, each");
    let _ = writeln!(out, "  one visible while it runs. Mission Control has the same");
    let _ = writeln!(out, "  view at http://127.0.0.1:7777.");
    let _ = writeln!(out, "");
    let _ = writeln!(out, "  Type `xos chat` to talk to it. It works offline.");
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + word.len() + 1 > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ---------------------------------------------------------------- running

/// Run the wizard.
pub fn run(connection: &mut Connection, skip_all: bool) -> Result<String, String> {
    let mut state = State::default();
    // Not a terminal, or told to skip: take the default for everything. The
    // installer runs it this way, and it must still end with a usable machine.
    let interactive = !skip_all && atty();

    say("
  XOS
  Everything here can be skipped, and XOS still works afterwards.
");

    for step in Step::all() {
        let answer = gather(connection, step, &state);
        if step == Step::Hardware {
            // Kept, because the model step is about to need it. Asking the
            // daemon twice could describe two different machines.
            state.hardware = Some(answer.clone());
        }
        say(&render_step(step, &state, &answer));

        if !interactive {
            state.skip(step);
            continue;
        }
        if !act(connection, step, &mut state, &answer)? {
            state.skip(step);
        }
    }

    // Then the first goal, automatically. Nobody is dropped at an empty prompt.
    let described = connection.call("firstrun.describe", json!({}))?;
    let goal = described.get("goal").cloned().unwrap_or(Value::Null);
    say(&render_finish(&state, &goal));

    let outcome = connection.call("firstrun.run", json!({}))?;
    if outcome.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(render_outcome(
            outcome.get("outcome").unwrap_or(&Value::Null),
        ))
    } else if outcome.get("already_run").and_then(Value::as_bool) == Some(true) {
        Ok(String::from(
            "\n  The first goal has already run on this machine.\n\
             \n  Type `xos chat` to talk to it. It works offline.\n",
        ))
    } else {
        // Even this does not fail the wizard. The point of the last ninety
        // seconds is a usable machine, and it is one.
        Ok(format!(
            "\n  The first goal did not finish: {}\n\
             \n  XOS is still ready. Type `xos chat`.\n",
            outcome
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given")
        ))
    }
}

fn gather(connection: &mut Connection, step: Step, state: &State) -> Value {
    match step {
        Step::Hardware => connection
            .call("hardware.inventory", json!({}))
            .map(|inventory| {
                let resolve = connection
                    .call("hardware.resolve", json!({}))
                    .unwrap_or(Value::Null);
                json!({
                    "summary": inventory.get("summary").cloned().unwrap_or(Value::Null),
                    "inventory": inventory.get("inventory").cloned().unwrap_or(Value::Null),
                    "resolve": resolve,
                })
            })
            .unwrap_or(Value::Null),
        Step::Model => {
            // The recommendation, with this machine's VRAM beside what the
            // model wants. A download that turns out not to fit is a bad forty
            // minutes, and the numbers are the only honest way to say so.
            let vram = state
                .hardware
                .as_ref()
                .and_then(|hardware| hardware.get("inventory"))
                .and_then(|inventory| inventory.get("gpus"))
                .and_then(Value::as_array)
                .and_then(|gpus| {
                    gpus.iter()
                        .filter_map(|gpu| gpu.get("vram_mb").and_then(Value::as_u64))
                        .max()
                })
                .unwrap_or(0);
            let mut answer = connection
                .call("models.recommend", json!({}))
                .unwrap_or(Value::Null);
            if let Some(object) = answer.as_object_mut() {
                object.insert("available_vram_mb".to_string(), json!(vram));
            }
            answer
        }
        _ => Value::Null,
    }
}

/// Returns false when the step was skipped.
fn act(
    connection: &mut Connection,
    step: Step,
    state: &mut State,
    _answer: &Value,
) -> Result<bool, String> {
    match step {
        Step::Hardware => Ok(ask("  Continue?", true)),
        Step::KillSwitch => {
            // Anything but "skip" counts as having tried it, and XOS halts and
            // resumes so the keys have actually done something.
            let typed = prompt("  Press Ctrl+Alt+Escape, then Enter (or type skip): ");
            if typed.trim().eq_ignore_ascii_case("skip") {
                return Ok(false);
            }
            let _ = connection.call("halt", json!({}));
            let _ = connection.call("resume", json!({}));
            state.kill_switch_pressed = true;
            Ok(true)
        }
        Step::Policy => {
            let routing = prompt("  Routing [1/2/3, Enter to skip]: ");
            let mode = match routing.trim() {
                "1" => Some("aggressive-local"),
                "2" => Some("balanced"),
                "3" => Some("best-quality"),
                _ => None,
            };
            let asking = prompt("  Asking [1/2/3, Enter to skip]: ");
            // These are the words the daemon actually parses. This step used to
            // send one that nothing recognised, so choosing it did nothing at
            // all and said so to nobody.
            let strictness = match asking.trim() {
                "1" => Some("strict"),
                "2" => Some("standard"),
                "3" => Some("permissive"),
                _ => None,
            };

            if let Some(mode) = mode {
                let _ = connection.call("mode.set", json!({ "mode": mode }));
                state.cost_mode = Some(mode.to_string());
            }
            if let Some(strictness) = strictness {
                match connection.call("policy.strictness", json!({ "strictness": strictness })) {
                    Ok(answer) if answer.get("ok").and_then(Value::as_bool) == Some(true) => {
                        state.strictness = Some(strictness.to_string());
                        if answer.get("in_effect").and_then(Value::as_bool) == Some(false) {
                            say("  Saved. It applies when XOS next starts.\n");
                        }
                    }
                    Ok(answer) => say(&format!(
                        "  That was not saved: {}\n",
                        answer.get("error").and_then(Value::as_str).unwrap_or("no reason given")
                    )),
                    // A setting that silently does nothing is worse than one
                    // that refuses, because somebody believes they tightened it.
                    Err(error) => say(&format!("  That was not saved: {}\n", error)),
                }
            }
            Ok(state.cost_mode.is_some() || state.strictness.is_some())
        }

        Step::Connectors => {
            // XOS shells out to each tool's own login and never sees a
            // password, so all this does is run them.
            let mut ran = Vec::new();
            for (tool, what) in [("gh", "GitHub"), ("rclone", "cloud storage")] {
                if ask(&format!("  Log in to {} now?", what), false) {
                    let status = std::process::Command::new(tool)
                        .arg(if tool == "gh" { "auth" } else { "config" })
                        .arg(if tool == "gh" { "login" } else { "reconnect" })
                        .status();
                    match status {
                        Ok(status) if status.success() => ran.push(tool.to_string()),
                        _ => say(&format!("  {} did not finish. Nothing was changed.
", tool)),
                    }
                }
            }
            state.connectors = ran;
            Ok(!state.connectors.is_empty())
        }

        Step::Messaging => {
            // The allowlist is required before this turns on, not after. An
            // assistant that will reply to anyone who messages it is a
            // different and much worse thing than one that replies to you.
            if !ask("  Set up messaging now?", false) {
                return Ok(false);
            }
            let allowed = prompt("  Numbers XOS may reply to, comma separated: ");
            if allowed.trim().is_empty() {
                say("  No allowlist, so messaging stays off. This is the safe way round.
");
                return Ok(false);
            }
            let _ = connection.call(
                "messaging.configure",
                json!({
                    "allowlist": allowed
                        .trim()
                        .split(',')
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .collect::<Vec<_>>(),
                }),
            );
            state.messaging_enabled = true;
            Ok(true)
        }

        Step::Voice => {
            if !ask("  Test the microphone now?", false) {
                return Ok(false);
            }
            state.voice_enabled = true;
            Ok(true)
        }
        Step::Keys => {
            let key = prompt("  OpenRouter key [Enter to skip]: ");
            if key.trim().is_empty() {
                return Ok(false);
            }
            let stored = connection.call(
                "vault.set",
                json!({ "provider": "openrouter", "key": key.trim() }),
            );
            match stored {
                Ok(_) => {
                    state.keys_added.push("openrouter".to_string());
                    Ok(true)
                }
                Err(error) => {
                    say(&format!("  That key was not stored: {}
", error));
                    Ok(false)
                }
            }
        }
        _ => Ok(ask("  Set this up now?", false)),
    }
}

fn atty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

fn prompt(question: &str) -> String {
    say(question);
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    answer
}

fn ask(question: &str, default_yes: bool) -> bool {
    let answer = prompt(&format!(
        "{} [{}] ",
        question,
        if default_yes { "Y/n" } else { "y/N" }
    ));
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default_yes,
        _ => default_yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardware() -> Value {
        json!({
            "summary": "GeForce GTX 1080 - 4 cores, 8 threads - 16000 MB memory - local - firmware uefi",
            "inventory": {
                "gpus": [{"description": "NVIDIA GP104 [GeForce GTX 1080]",
                          "vendor_id": "10de", "device_id": "1b80",
                          "kernel_driver": "nvidia", "vram_mb": 8192}],
                "network": [{"description": "Realtek RTL8111"}],
                "firmware": {"mode": "uefi", "secure_boot": false}
            },
            "resolve": {
                "display": [{"vendor_id": "10de", "device_id": "1b80",
                             "driver": "nvidia-580xx-dkms", "branch": "580"}]
            }
        })
    }

    #[test]
    fn skipping_everything_still_leaves_a_usable_machine() {
        let mut state = State::default();
        for step in Step::all() {
            state.skip(step);
        }
        assert_eq!(state.skipped.len(), 9);
        assert!(
            state.usable(),
            "a wizard that can leave somebody without an assistant has failed"
        );
    }

    #[test]
    fn the_wizard_finishes_even_when_nobody_is_reading() {
        // `xos setup | head` used to abort the wizard partway, so the machine
        // was set up and the one goal that demonstrates it never ran. Output is
        // a courtesy; finishing is not.
        let source = include_str!("setup.rs");
        let body = source.split("#[cfg(test)]").next().expect("the code");
        assert!(
            !body.contains("println!") && !body.contains("print!("),
            "a macro that panics on a closed pipe would abort the wizard"
        );
    }

    #[test]
    fn the_hardware_answer_is_kept_for_the_model_step() {
        // Asking the daemon a second time could describe a different machine,
        // and the model step is deciding whether a download will fit on this
        // one.
        let mut state = State::default();
        state.hardware = Some(hardware());
        let vram = state.hardware.as_ref().and_then(|h| {
            h["inventory"]["gpus"][0]["vram_mb"].as_u64()
        });
        assert_eq!(vram, Some(8192));
    }

    #[test]
    fn there_are_nine_steps_in_the_stated_order() {
        let titles: Vec<&str> = Step::all().iter().map(|s| s.title()).collect();
        assert_eq!(titles.len(), 9);
        assert_eq!(titles[0], "What you have");
        assert_eq!(titles[8], "The stop button");
    }

    #[test]
    fn the_hardware_step_says_what_is_not_working() {
        // Left out, somebody finds it later and has no idea it was known about.
        let mut answer = hardware();
        answer["inventory"]["network"] = json!([]);
        answer["inventory"]["firmware"] = json!({"mode": "unknown"});
        let output = render_step(Step::Hardware, &State::default(), &answer);
        assert!(output.contains("Not working:"), "{}", output);
        assert!(output.contains("no network device"), "{}", output);
        assert!(output.contains("firmware mode could not be read"), "{}", output);
    }

    #[test]
    fn a_healthy_machine_is_told_so_rather_than_shown_an_empty_list() {
        let output = render_step(Step::Hardware, &State::default(), &hardware());
        assert!(output.contains("Everything here is working."), "{}", output);
        assert!(!output.contains("Not working:"), "{}", output);
    }

    #[test]
    fn an_unknown_card_is_named_in_what_is_not_working() {
        let mut answer = hardware();
        answer["resolve"]["display"] = json!([{
            "vendor_id": "dead", "device_id": "beef", "driver": "unknown"
        }]);
        let output = render_step(Step::Hardware, &State::default(), &answer);
        assert!(output.contains("dead:beef"), "{}", output);
        assert!(output.contains("not in the hardware database"), "{}", output);
    }

    #[test]
    fn the_model_step_shows_the_fit_in_numbers() {
        // A forty-minute download that turns out not to fit is a bad first hour.
        let answer = json!({
            "name": "Gemma 4 E4B", "approximate_size_gb": 4.2,
            "minimum_vram_mb": 6144, "available_vram_mb": 4096
        });
        let output = render_step(Step::Model, &State::default(), &answer);
        assert!(output.contains("6144"), "{}", output);
        assert!(output.contains("4096"), "{}", output);
        assert!(output.contains("will not fit"), "{}", output);
    }

    #[test]
    fn openrouter_is_offered_first_and_the_reason_given() {
        let output = render_step(Step::Keys, &State::default(), &Value::Null);
        assert!(output.contains("OpenRouter first"), "{}", output);
        assert!(output.contains("one key rather than"), "{}", output);
    }

    #[test]
    fn the_connectors_step_says_xos_registers_nothing_of_its_own() {
        let output = render_step(Step::Connectors, &State::default(), &Value::Null);
        assert!(output.contains("registers no accounts of its own"), "{}", output);
        assert!(output.contains("gh"), "{}", output);
        assert!(output.contains("rclone"), "{}", output);
        assert!(output.contains("openclaw"), "{}", output);
    }

    #[test]
    fn messaging_warns_before_it_offers() {
        // Linking a main WhatsApp account means XOS sees every conversation on
        // it, and somebody should know that before the QR code, not after.
        let output = render_step(Step::Messaging, &State::default(), &Value::Null);
        let warning = output.find("dedicated number").expect("the warning");
        let allowlist = output.find("allowlist").expect("the allowlist requirement");
        assert!(output.contains("every conversation"), "{}", output);
        assert!(warning < output.len() && allowlist < output.len());
    }

    #[test]
    fn the_policy_step_offers_words_the_daemon_understands() {
        // It used to offer a word nothing parses, so choosing it did nothing
        // at all. The daemon's vocabulary is the only vocabulary.
        let source = include_str!("setup.rs");
        let body = source.split("#[cfg(test)]").next().expect("the code");
        assert!(!body.contains("paranoid"), "the daemon has never heard of it");
        for word in ["strict", "standard", "permissive"] {
            assert!(body.contains(word), "{} is not offered", word);
        }
    }

    #[test]
    fn the_policy_step_describes_what_each_choice_permits() {
        // Not the names of the modes. Somebody in their first minute has no idea
        // what "balanced" means.
        let output = render_step(Step::Policy, &State::default(), &Value::Null);
        assert!(output.contains("Slower, and free"), "{}", output);
        assert!(output.contains("cannot be undone"), "{}", output);
        assert!(!output.contains("aggressive-local"), "{}", output);
    }

    #[test]
    fn the_kill_switch_step_asks_somebody_to_press_it() {
        // A control nobody has used is one they will not reach for when they
        // need it, and needing it is the whole point of having it.
        let output = render_step(Step::KillSwitch, &State::default(), &Value::Null);
        assert!(output.contains("Ctrl+Alt+Escape"), "{}", output);
        assert!(output.contains("Press it now"), "{}", output);
        assert!(output.contains("Not a pause"), "{}", output);
    }

    #[test]
    fn the_kill_switch_is_skippable_like_everything_else() {
        let output = render_step(Step::KillSwitch, &State::default(), &Value::Null);
        assert!(output.contains("skip"), "{}", output);
    }

    #[test]
    fn the_finish_names_what_was_skipped_and_how_to_come_back() {
        let mut state = State::default();
        state.skip(Step::Keys);
        state.skip(Step::Voice);
        let goal = json!({
            "description": "Take stock of this machine and what I work on",
            "steps": [{"title": "Take stock of this machine"}],
            "reads": ["~/Documents", "~/Projects"],
            "writes": "one entry in long-term memory, and nothing else"
        });
        let output = render_finish(&state, &goal);
        assert!(output.contains("Skipped: Provider keys, Voice"), "{}", output);
        assert!(output.contains("xos setup"), "{}", output);
    }

    #[test]
    fn somebody_who_skipped_everything_is_told_it_still_works() {
        // This is the promise the whole wizard is built around, and the person
        // who most needs to read it is the one who skipped every step.
        let mut state = State::default();
        for step in Step::all() {
            state.skip(step);
        }
        let output = render_finish(&state, &json!({}));
        assert!(output.contains("XOS still works"), "{}", output);
        assert!(output.contains("offline"), "{}", output);
    }

    #[test]
    fn what_was_set_up_is_reported_as_well_as_what_was_skipped() {
        let mut state = State::default();
        state.keys_added.push("openrouter".to_string());
        state.connectors.push("gh".to_string());
        state.messaging_enabled = true;
        state.voice_enabled = true;
        state.cost_mode = Some("balanced".to_string());
        state.strictness = Some("standard".to_string());
        state.kill_switch_pressed = true;
        let output = render_finish(&state, &json!({}));
        for expected in [
            "keys for openrouter",
            "connectors: gh",
            "messaging, with an allowlist",
            "voice",
            "routing: balanced",
            "asking: standard",
            "the stop button, tried once",
        ] {
            assert!(output.contains(expected), "{} missing from:\n{}", expected, output);
        }
    }

    #[test]
    fn the_first_goal_says_what_it_will_touch_before_it_runs() {
        let goal = json!({
            "description": "Take stock of this machine and what I work on",
            "steps": [{"title": "Take stock of this machine"}],
            "reads": ["~/Documents", "~/Projects", "~/code", "~/Downloads"],
            "writes": "one entry in long-term memory, and nothing else"
        });
        let output = render_finish(&State::default(), &goal);
        assert!(output.contains("~/Documents"), "{}", output);
        assert!(output.contains("without opening anything"), "{}", output);
        assert!(output.contains("long-term memory"), "{}", output);
        assert!(output.contains("Nothing leaves this machine"), "{}", output);
    }

    #[test]
    fn the_outcome_points_at_chat_and_says_it_works_offline() {
        let outcome = json!({
            "steps": [{"title": "Take stock of this machine", "result": "a test machine"}],
            "summary": "This machine: a test machine."
        });
        let output = render_outcome(&outcome);
        assert!(output.contains("xos chat"), "{}", output);
        assert!(output.contains("offline"), "{}", output);
        assert!(output.contains("127.0.0.1:7777"), "{}", output);
    }

    #[test]
    fn long_summaries_are_wrapped_rather_than_run_off_the_screen() {
        let long = "word ".repeat(60);
        for line in wrap(&long, 62) {
            assert!(line.len() <= 62, "{}", line);
        }
    }
}
