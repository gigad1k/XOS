//! XOS Bench — model validation harness.
//!
//! Measures the metric that decides the architecture: tool-call schema success
//! rate. A model that generates quickly but emits malformed tool calls forces
//! constant escalation and costs API rates anyway, so tokens per second is
//! reported alongside rather than on its own.
//!
//! Read-only against the endpoint. The only file written is the result file.

mod builtin;
mod case;
mod client;
mod eval;
mod gpu;
mod report;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use case::{Case, Expect};
use client::{Client, Turn};
use report::CaseResult;

#[derive(Parser, Debug)]
#[command(
    name = "xos-bench",
    about = "Measure whether a local model can drive tools reliably"
)]
struct Args {
    /// Base URL of an OpenAI-compatible endpoint.
    #[arg(long, default_value = "http://localhost:11434/v1")]
    url: String,

    /// Model name to request.
    #[arg(long, default_value = "gemma4:e4b")]
    model: String,

    /// Load cases from a JSON file instead of the built-in suite.
    #[arg(long)]
    cases: Option<PathBuf>,

    /// Where to write the JSON result file.
    #[arg(long, default_value = "xos-bench-results.json")]
    out: PathBuf,

    /// Run only the first N cases.
    #[arg(long)]
    limit: Option<usize>,

    /// Run only one category: single-call, multi-turn, nested-args,
    /// numeric-arg or refusal.
    #[arg(long)]
    category: Option<String>,

    /// Per-request timeout in seconds.
    #[arg(long, default_value_t = 120)]
    timeout: u64,

    /// Request whole replies instead of streaming. Loses time to first token.
    #[arg(long)]
    no_stream: bool,

    /// Print the built-in suite as JSON and exit, as a starting point for a
    /// custom case file.
    #[arg(long)]
    dump_cases: bool,

    /// Skip the warm-up request. Without it the first case pays for loading
    /// the model into VRAM, which lands a cold-start figure on one case.
    #[arg(long)]
    no_warmup: bool,

    /// Where results would be sent. Prints the destination and sends nothing.
    #[arg(long)]
    submit: bool,

    /// Bearer token, for endpoints that want one.
    #[arg(long, env = "XOS_BENCH_API_KEY")]
    api_key: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    if args.dump_cases {
        match serde_json::to_string_pretty(&builtin::suite()) {
            Ok(text) => {
                println!("{}", text);
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("xos-bench: cannot serialise the built-in suite: {}", error);
                return ExitCode::FAILURE;
            }
        }
    }

    let mut cases = match &args.cases {
        Some(path) => match case::load(path) {
            Ok(cases) => cases,
            Err(error) => {
                eprintln!("xos-bench: {}", error);
                return ExitCode::FAILURE;
            }
        },
        None => builtin::suite(),
    };

    if let Some(category) = &args.category {
        cases.retain(|c| c.category.label() == category);
        if cases.is_empty() {
            eprintln!(
                "xos-bench: no cases in category `{}`. Categories are: single-call, multi-turn, nested-args, numeric-arg, refusal.",
                category
            );
            return ExitCode::FAILURE;
        }
    }
    if let Some(limit) = args.limit {
        cases.truncate(limit);
    }

    let client = Client::new(
        &args.url,
        &args.model,
        args.api_key.clone(),
        Duration::from_secs(args.timeout),
        !args.no_stream,
    );

    eprintln!(
        "xos-bench: {} cases against {} at {}",
        cases.len(),
        args.model,
        client.endpoint()
    );

    if !args.no_warmup {
        eprintln!("xos-bench: warming the model");
        let _ = client.complete(&[Turn::User("Reply with the word ready.".to_string())], &[]);
    }

    let watch = gpu::VramWatch::start();
    let total = cases.len();
    let mut results = Vec::with_capacity(total);
    for (index, case) in cases.iter().enumerate() {
        let result = run_case(&client, case);
        eprintln!(
            "[{:>3}/{}] {:<28} {}",
            index + 1,
            total,
            result.id,
            if result.passed {
                "ok".to_string()
            } else {
                format!("failed: {}", result.note)
            }
        );
        results.push(result);
    }
    let peak_vram_mb = watch.map(|w| w.finish());

    let report = report::build(client.model(), &client.endpoint(), results, peak_vram_mb);
    report::print_table(&report);

    match write_results(&args.out, &report) {
        Ok(()) => println!("results written to {}", args.out.display()),
        Err(error) => {
            eprintln!("xos-bench: {}", error);
            return ExitCode::FAILURE;
        }
    }

    if args.submit {
        println!(
            "submit: results would go to the XOS hardware compatibility list at https://github.com/gigad1k/XOS. Nothing was sent; this flag is a stub."
        );
    }

    ExitCode::SUCCESS
}

fn write_results(path: &PathBuf, report: &report::Report) -> Result<(), String> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| format!("cannot serialise results: {}", e))?;
    let mut file =
        fs::File::create(path).map_err(|e| format!("cannot create {}: {}", path.display(), e))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

/// Run one case end to end and score it.
fn run_case(client: &Client, case: &Case) -> CaseResult {
    let mut result = CaseResult {
        id: case.id.clone(),
        category: case.category.label().to_string(),
        schema_valid: false,
        args_plausible: false,
        passed: false,
        steps_expected: case.steps().len(),
        steps_completed: 0,
        latency_ms: 0,
        ttft_ms: None,
        tokens_in: None,
        tokens_out: None,
        tokens_estimated: false,
        note: String::new(),
    };

    match &case.expect {
        Expect::NoCall => {
            let completion = match client.complete(&[Turn::User(case.prompt.clone())], &case.tools) {
                Ok(completion) => completion,
                Err(error) => {
                    result.note = error;
                    return result;
                }
            };
            absorb(&mut result, &completion);

            if completion.tool_calls.is_empty() {
                // Nothing to dispatch, which is the correct move here.
                result.schema_valid = true;
                result.args_plausible = true;
                result.passed = true;
            } else {
                // A well-formed call is still well-formed; the decision is what
                // went wrong, so schema validity is reported on its own terms.
                let call = &completion.tool_calls[0];
                result.schema_valid = eval::schema_valid(call, &case.tools).is_ok();
                result.note = format!("called `{}` when no tool applies", call.name);
            }
            result
        }
        Expect::Sequence { steps } => {
            let mut turns = vec![Turn::User(case.prompt.clone())];
            let mut schema_ok_everywhere = true;

            for (index, step) in steps.iter().enumerate() {
                let completion = match client.complete(&turns, &case.tools) {
                    Ok(completion) => completion,
                    Err(error) => {
                        result.note = error;
                        schema_ok_everywhere = false;
                        break;
                    }
                };
                absorb(&mut result, &completion);

                let call = match completion.tool_calls.first() {
                    Some(call) => call.clone(),
                    None => {
                        result.note = format!(
                            "step {} emitted no tool call{}",
                            index + 1,
                            said(&completion.content)
                        );
                        schema_ok_everywhere = false;
                        break;
                    }
                };

                let arguments = match eval::schema_valid(&call, &case.tools) {
                    Ok(arguments) => arguments,
                    Err(error) => {
                        result.note = format!("step {}: {}", index + 1, error);
                        schema_ok_everywhere = false;
                        break;
                    }
                };

                if let Err(error) = eval::args_plausible(&call, &arguments, step) {
                    result.note = format!("step {}: {}", index + 1, error);
                    break;
                }

                result.steps_completed += 1;

                if index + 1 < steps.len() {
                    let feedback = step
                        .result
                        .clone()
                        .unwrap_or_else(|| "{\"ok\":true}".to_string());
                    turns.push(Turn::AssistantCall(call.clone()));
                    turns.push(Turn::ToolResult {
                        call_id: call.id.clone(),
                        content: feedback,
                    });
                }
            }

            result.schema_valid = schema_ok_everywhere;
            result.args_plausible = result.steps_completed == steps.len();
            result.passed = result.args_plausible && schema_ok_everywhere;
            result
        }
    }
}

/// Quote what the model said instead of calling a tool, to make a failure
/// readable without rerunning it.
fn said(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let snippet: String = trimmed.chars().take(80).collect();
        format!(", it said: {}", snippet)
    }
}

/// Fold one request's timings into the case total.
fn absorb(result: &mut CaseResult, completion: &client::Completion) {
    result.latency_ms += completion.latency_ms;
    if result.ttft_ms.is_none() {
        result.ttft_ms = completion.ttft_ms;
    }
    if let Some(tokens) = completion.tokens_in {
        result.tokens_in = Some(result.tokens_in.unwrap_or(0) + tokens);
    }
    if let Some(tokens) = completion.tokens_out {
        result.tokens_out = Some(result.tokens_out.unwrap_or(0) + tokens);
    }
    result.tokens_estimated |= completion.tokens_estimated;
}
