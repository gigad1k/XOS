//! Results, aggregation and output.
//!
//! The table is the thing a person reads; the JSON file is the thing a later
//! run compares against. Both carry the same numbers.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::case::Category;

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub category: String,
    /// The model emitted something the runtime could dispatch.
    pub schema_valid: bool,
    /// It chose the expected tool and filled it in sensibly.
    pub args_plausible: bool,
    /// The case as a whole succeeded.
    pub passed: bool,
    pub steps_expected: usize,
    pub steps_completed: usize,
    pub latency_ms: u128,
    pub ttft_ms: Option<u128>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    /// True when the endpoint reported no usage and counts were estimated.
    pub tokens_estimated: bool,
    pub note: String,
}

impl CaseResult {
    /// Decode rate, excluding the wait for the first token, which is prefill
    /// rather than generation.
    ///
    /// Returns nothing when the decode window is too short to divide by. Short
    /// tool-call replies often arrive in a single burst, leaving a window of a
    /// millisecond or two; dividing by that yields thousands of tokens per
    /// second, which is an artefact of the measurement rather than the model.
    pub fn tokens_per_second(&self) -> Option<f64> {
        let tokens = self.tokens_out? as f64;
        if tokens <= 0.0 {
            return None;
        }
        let ttft = self.ttft_ms.unwrap_or(0);
        let decode_ms = self.latency_ms.saturating_sub(ttft) as f64;
        if decode_ms < MIN_DECODE_WINDOW_MS {
            return None;
        }
        Some(tokens / (decode_ms / 1000.0))
    }

    /// Tokens per second across the whole request, prefill included. Always
    /// measurable, and the figure that reflects what a user waits for.
    pub fn throughput(&self) -> Option<f64> {
        let tokens = self.tokens_out? as f64;
        if tokens <= 0.0 || self.latency_ms == 0 {
            return None;
        }
        Some(tokens / (self.latency_ms as f64 / 1000.0))
    }
}

/// Below this, the decode window is noise rather than signal.
const MIN_DECODE_WINDOW_MS: f64 = 50.0;

#[derive(Debug, Clone, Serialize)]
pub struct CategorySummary {
    pub category: String,
    pub cases: usize,
    pub passed: usize,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub cases: usize,
    pub errors: usize,
    /// The decisive metric: share of cases where the model drove the tool right.
    pub tool_call_success_percent: f64,
    /// Share where the emitted call was at least dispatchable.
    pub schema_valid_percent: f64,
    pub multi_turn_completion_percent: f64,
    /// Decode rate, averaged over cases whose decode window was long enough to
    /// measure. `decode_rate_cases` says how many that was.
    pub mean_tokens_per_second: Option<f64>,
    pub decode_rate_cases: usize,
    /// End-to-end tokens per second, prefill included, over every case.
    pub mean_throughput_tokens_per_second: Option<f64>,
    pub mean_ttft_ms: Option<f64>,
    pub peak_vram_mb: Option<u64>,
    pub tokens_estimated: bool,
    pub by_category: Vec<CategorySummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub model: String,
    pub endpoint: String,
    pub generated_unix: u64,
    pub summary: Summary,
    pub cases: Vec<CaseResult>,
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64) * 100.0 / (whole as f64)
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

pub fn summarise(results: &[CaseResult], peak_vram_mb: Option<u64>) -> Summary {
    let cases = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let schema_ok = results.iter().filter(|r| r.schema_valid).count();
    let errors = results.iter().filter(|r| !r.note.is_empty() && !r.passed).count();

    let multi: Vec<&CaseResult> = results
        .iter()
        .filter(|r| r.category == Category::MultiTurn.label())
        .collect();
    let multi_done = multi
        .iter()
        .filter(|r| r.steps_expected > 0 && r.steps_completed == r.steps_expected)
        .count();

    let rates: Vec<f64> = results.iter().filter_map(|r| r.tokens_per_second()).collect();
    let throughputs: Vec<f64> = results.iter().filter_map(|r| r.throughput()).collect();
    let ttfts: Vec<f64> = results
        .iter()
        .filter_map(|r| r.ttft_ms.map(|ms| ms as f64))
        .collect();

    let by_category = Category::all()
        .iter()
        .map(|category| {
            let label = category.label();
            let group: Vec<&CaseResult> = results.iter().filter(|r| r.category == label).collect();
            let group_passed = group.iter().filter(|r| r.passed).count();
            CategorySummary {
                category: label.to_string(),
                cases: group.len(),
                passed: group_passed,
                percent: percent(group_passed, group.len()),
            }
        })
        .collect();

    Summary {
        cases,
        errors,
        tool_call_success_percent: percent(passed, cases),
        schema_valid_percent: percent(schema_ok, cases),
        multi_turn_completion_percent: percent(multi_done, multi.len()),
        mean_tokens_per_second: mean(&rates),
        decode_rate_cases: rates.len(),
        mean_throughput_tokens_per_second: mean(&throughputs),
        mean_ttft_ms: mean(&ttfts),
        peak_vram_mb,
        tokens_estimated: results.iter().any(|r| r.tokens_estimated),
        by_category,
    }
}

pub fn build(model: &str, endpoint: &str, results: Vec<CaseResult>, peak_vram_mb: Option<u64>) -> Report {
    let summary = summarise(&results, peak_vram_mb);
    Report {
        model: model.to_string(),
        endpoint: endpoint.to_string(),
        generated_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        summary,
        cases: results,
    }
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "--"
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width.saturating_sub(1)).collect::<String>() + "\u{2026}"
    }
}

/// Print the human table. Dense, aligned, no decoration.
pub fn print_table(report: &Report) {
    println!();
    println!("XOS Bench");
    println!("model     {}", report.model);
    println!("endpoint  {}", report.endpoint);
    println!();
    println!(
        "{:<28} {:<12} {:>5} {:>5} {:>7} {:>8} {:>8} {:>8}",
        "case", "category", "valid", "args", "steps", "ttft ms", "total ms", "tok/s"
    );
    println!("{}", "-".repeat(88));

    for result in &report.cases {
        let steps = format!("{}/{}", result.steps_completed, result.steps_expected);
        let ttft = result
            .ttft_ms
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| "-".to_string());
        let rate = result
            .tokens_per_second()
            .map(|r| format!("{:.1}", r))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<28} {:<12} {:>5} {:>5} {:>7} {:>8} {:>8} {:>8}",
            truncate(&result.id, 28),
            result.category,
            mark(result.schema_valid),
            mark(result.args_plausible),
            steps,
            ttft,
            result.latency_ms,
            rate
        );
    }

    let failures: Vec<&CaseResult> = report.cases.iter().filter(|r| !r.passed).collect();
    if !failures.is_empty() {
        println!();
        println!("failures");
        for result in failures {
            println!("  {:<28} {}", truncate(&result.id, 28), result.note);
        }
    }

    let summary = &report.summary;
    println!();
    println!("{}", "-".repeat(88));
    println!(
        "tool-call success        {:.1}%  ({} of {} cases)",
        summary.tool_call_success_percent,
        report.cases.iter().filter(|r| r.passed).count(),
        summary.cases
    );
    println!("schema-valid             {:.1}%", summary.schema_valid_percent);
    println!(
        "multi-turn completion    {:.1}%",
        summary.multi_turn_completion_percent
    );
    let estimated = if summary.tokens_estimated {
        "  (token counts estimated)"
    } else {
        ""
    };
    match summary.mean_throughput_tokens_per_second {
        Some(rate) => println!("mean tok/s end to end    {:.1}{}", rate, estimated),
        None => println!("mean tok/s end to end    no generation measured"),
    }
    match summary.mean_tokens_per_second {
        Some(rate) => println!(
            "mean tok/s decoding      {:.1}  (over {} of {} cases)",
            rate, summary.decode_rate_cases, summary.cases
        ),
        None => println!(
            "mean tok/s decoding      replies arrived in one burst, decode window too short to measure"
        ),
    }
    match summary.mean_ttft_ms {
        Some(ttft) => println!("mean time to first token {:.0} ms", ttft),
        None => println!("mean time to first token not measured"),
    }
    match summary.peak_vram_mb {
        Some(vram) => println!("peak VRAM                {} MB", vram),
        None => println!("peak VRAM                nvidia-smi not available"),
    }

    println!();
    println!("{:<14} {:>6} {:>8}", "category", "passed", "percent");
    for group in &summary.by_category {
        println!(
            "{:<14} {:>6} {:>7.1}%",
            group.category,
            format!("{}/{}", group.passed, group.cases),
            group.percent
        );
    }
    println!();
}
