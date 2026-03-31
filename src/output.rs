//! Beautiful, structured terminal output for burst-forge.
//!
//! Provides consistent formatting with visual hierarchy, dot-leader alignment,
//! ANSI color (when stdout is a TTY), timing, and ASCII table rendering.

#![allow(dead_code)]

use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::LazyLock;

/// Whether stdout is a TTY (enables ANSI colors).
static IS_TTY: LazyLock<bool> = LazyLock::new(|| std::io::stdout().is_terminal());

// ---------------------------------------------------------------------------
// ANSI color helpers
// ---------------------------------------------------------------------------

/// ANSI escape: bold text.
#[must_use]
pub fn bold(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: green text.
#[must_use]
pub fn green(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: red text.
#[must_use]
pub fn red(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: yellow text.
#[must_use]
pub fn yellow(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: dim/gray text.
#[must_use]
pub fn dim(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: bold green text.
#[must_use]
pub fn bold_green(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[1;32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: bold red text.
#[must_use]
pub fn bold_red(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[1;31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: bold yellow text.
#[must_use]
pub fn bold_yellow(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[1;33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// ANSI escape: cyan text.
#[must_use]
pub fn cyan(s: &str) -> String {
    if *IS_TTY {
        format!("\x1b[36m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Banner
// ---------------------------------------------------------------------------

/// Print the burst-forge startup banner with version.
pub fn print_banner(subtitle: &str) {
    let version = env!("CARGO_PKG_VERSION");
    let inner = format!("  burst-forge v{version} -- {subtitle}  ");
    let width = inner.len();
    let top = format!("\u{2554}{}\u{2557}", "\u{2550}".repeat(width));
    let mid = format!("\u{2551}{inner}\u{2551}");
    let bot = format!("\u{255a}{}\u{255d}", "\u{2550}".repeat(width));
    println!();
    println!("{}", bold(&top));
    println!("{}", bold(&mid));
    println!("{}", bold(&bot));
    println!();
}

// ---------------------------------------------------------------------------
// Section headers
// ---------------------------------------------------------------------------

/// Print a major phase header.
///
/// ```text
/// === Phase: Node Pre-Heat (19 nodes) ===
/// ```
pub fn print_phase(label: &str) {
    println!();
    println!("{}", bold(&format!("=== {label} ===")));
    println!();
}

/// Print a phase header with elapsed time.
///
/// ```text
/// === Node Pre-Heat (completed in 2m 34s) ===
/// ```
pub fn print_phase_complete(label: &str, elapsed_secs: u64) {
    println!();
    println!(
        "{}",
        bold(&format!("=== {label} (completed in {}) ===", format_duration(elapsed_secs)))
    );
    println!();
}

/// Print a scenario separator.
///
/// ```text
/// --- Scenario 1/6: 751-1000 pods ---
/// ```
pub fn print_scenario(index: usize, total: usize, name: &str, replicas: u32, gw: u32, wh: u32) {
    println!();
    println!(
        "{}",
        bold(&format!(
            "--- Scenario {}/{total}: {name} (replicas={replicas}, gw={gw}, wh={wh}) ---",
            index + 1
        ))
    );
}

/// Print a sub-section label.
pub fn print_subsection(label: &str) {
    println!();
    println!("  {}", bold(label));
}

// ---------------------------------------------------------------------------
// Gate output with dot-leader alignment
// ---------------------------------------------------------------------------

/// Width for dot-leader alignment (gate label + dots + status).
const DOT_LEADER_WIDTH: usize = 48;

/// Print a gate result with dot-leader alignment and PASS/FAIL coloring.
///
/// ```text
///   [Gate 1] Node Ready ................... 19/19 PASS
///   [Gate 3] Infrastructure .............. FAIL
///     Expected: GW 6/6 ready
///     Actual:   GW 4/6 ready (2 pods not ready after 180s)
/// ```
pub fn print_gate_result(gate: &str, detail: &str, passed: bool) {
    let label = format!("  {gate} {detail}");
    let status = if passed {
        bold_green("PASS")
    } else {
        bold_red("FAIL")
    };
    let plain_label_len = strip_ansi_len(&label);
    let dots_needed = if plain_label_len < DOT_LEADER_WIDTH {
        DOT_LEADER_WIDTH - plain_label_len
    } else {
        3
    };
    let dots = dim(&".".repeat(dots_needed));
    println!("{label} {dots} {status}");
}

/// Print gate failure details (expected vs actual).
pub fn print_gate_failure_detail(expected: &str, actual: &str) {
    println!("    {}: {expected}", bold("Expected"));
    println!("    {}:   {actual}", bold("Actual"));
}

// ---------------------------------------------------------------------------
// Progress indicators
// ---------------------------------------------------------------------------

/// Print a timed progress line (non-overwriting).
///
/// ```text
///   [  7.4s] Ready+Schedulable nodes: 15/19
/// ```
pub fn print_progress(elapsed_secs: u64, message: &str) {
    println!("  [{elapsed_secs:>4}s] {message}");
}

/// Print a timed progress line with millisecond precision.
///
/// ```text
///   [  7400ms] Running: 320 | Pending: 680 | Injected: 1000
/// ```
pub fn print_progress_ms(elapsed_ms: u64, message: &str) {
    print!("\r  [{elapsed_ms:>6}ms] {message}");
}

/// Print a status line with indentation.
pub fn print_status(message: &str) {
    println!("  {message}");
}

/// Print an action being taken.
pub fn print_action(message: &str) {
    println!("  {}", dim(message));
}

/// Print a warning.
pub fn print_warning(message: &str) {
    eprintln!("  {}", bold_yellow(&format!("WARNING: {message}")));
}

/// Print the burst start line.
///
/// ```text
///   BURST: 0 -> 1000 replicas (maxSurge=1000)
/// ```
pub fn print_burst_start(replicas: u32) {
    println!();
    println!(
        "  {}",
        bold(&format!(
            "BURST: 0 -> {replicas} replicas (maxSurge={replicas})"
        ))
    );
    println!();
}

/// Print burst completion.
pub fn print_burst_complete(running: u32, replicas: u32, elapsed_ms: u64, injection_rate: f64) {
    println!();
    #[allow(clippy::cast_precision_loss)]
    let secs = elapsed_ms as f64 / 1000.0;
    let status = if running >= replicas {
        bold_green(&format!("{running}/{replicas} pods in {secs:.1}s"))
    } else {
        bold_yellow(&format!("{running}/{replicas} pods in {secs:.1}s"))
    };
    println!("  Result: {status} (injection: {injection_rate:.1}%)");
}

/// Print a timeout message.
pub fn print_timeout(timeout_secs: u64) {
    println!();
    println!("  {}", bold_red(&format!("TIMEOUT after {timeout_secs}s")));
}

/// Print capacity limit reached.
pub fn print_capacity_limit(running: u32, replicas: u32) {
    println!();
    println!(
        "  {}",
        bold_yellow(&format!(
            "CAPACITY LIMIT: {running}/{replicas} pods (no more schedulable)"
        ))
    );
}

// ---------------------------------------------------------------------------
// Iteration results
// ---------------------------------------------------------------------------

/// Print iteration results block.
pub fn print_iteration_results(
    iteration: u32,
    running: u32,
    requested: u32,
    injection_rate: f64,
    first_ready_ms: u64,
    all_ready_ms: Option<u64>,
    duration_ms: u64,
) {
    println!();
    println!("  {}", bold(&format!("Iteration {iteration} Results")));
    println!("  Pods Running:     {running}/{requested}");
    println!("  Injection Rate:   {injection_rate:.1}%");
    println!("  First Ready:      {first_ready_ms}ms");
    if let Some(all) = all_ready_ms {
        println!("  All Ready:        {all}ms");
    }
    println!("  Total Duration:   {duration_ms}ms");
}

// ---------------------------------------------------------------------------
// Drain output
// ---------------------------------------------------------------------------

/// Print drain progress.
pub fn print_drain_progress(elapsed_secs: u64, remaining: u32) {
    if remaining == 0 {
        println!(
            "  [{elapsed_secs:>4}s] Draining pods {} 0 remaining",
            dim(".............")
        );
    } else {
        println!(
            "  [{elapsed_secs:>4}s] Draining pods {} {remaining} remaining",
            dim(".............")
        );
    }
}

/// Print drain timeout with force delete.
pub fn print_drain_timeout(timeout_secs: u64, remaining: u32) {
    println!(
        "  {}",
        bold_yellow(&format!(
            "Drain timeout after {timeout_secs}s with {remaining} pods remaining -- force deleting"
        ))
    );
}

// ---------------------------------------------------------------------------
// Summary table
// ---------------------------------------------------------------------------

/// A row for the summary table.
pub struct SummaryRow {
    pub scenario: String,
    pub pods: String,
    pub time: String,
    pub injection: String,
    pub status: String,
}

/// Print an ASCII summary table for matrix results.
///
/// ```text
///   +-----------------+----------+---------+-----------+----------+
///   | Scenario        | Pods     | Time    | Injection | Status   |
///   +-----------------+----------+---------+-----------+----------+
///   | 751-1000 pods   | 1000/1000| 22.8s   | 100%      | PASS     |
///   +-----------------+----------+---------+-----------+----------+
/// ```
pub fn print_summary_table(rows: &[SummaryRow]) {
    // Calculate column widths
    let col_scenario = rows.iter().map(|r| r.scenario.len()).max().unwrap_or(8).max(8);
    let col_pods = rows.iter().map(|r| r.pods.len()).max().unwrap_or(4).max(4);
    let col_time = rows.iter().map(|r| r.time.len()).max().unwrap_or(4).max(4);
    let col_inj = rows.iter().map(|r| r.injection.len()).max().unwrap_or(9).max(9);
    let col_status = 6; // "Status" / "PASS" / "FAIL"

    let sep = format!(
        "  +-{}-+-{}-+-{}-+-{}-+-{}-+",
        "-".repeat(col_scenario),
        "-".repeat(col_pods),
        "-".repeat(col_time),
        "-".repeat(col_inj),
        "-".repeat(col_status),
    );

    // Header
    println!("{sep}");
    println!(
        "  | {:<col_scenario$} | {:<col_pods$} | {:<col_time$} | {:<col_inj$} | {:<col_status$} |",
        "Scenario", "Pods", "Time", "Injection", "Status"
    );
    println!("{sep}");

    // Rows
    for row in rows {
        let status_colored = if row.status == "PASS" {
            bold_green(&row.status)
        } else {
            bold_red(&row.status)
        };
        // For colored strings, we need to pad manually since ANSI escapes affect width
        let status_padding = col_status.saturating_sub(row.status.len());
        println!(
            "  | {:<col_scenario$} | {:<col_pods$} | {:<col_time$} | {:<col_inj$} | {status_colored}{} |",
            row.scenario, row.pods, row.time, row.injection,
            " ".repeat(status_padding),
        );
    }

    // Footer
    println!("{sep}");
}

// ---------------------------------------------------------------------------
// Node status
// ---------------------------------------------------------------------------

/// Print node group status in a structured block.
#[allow(clippy::too_many_arguments)]
pub fn print_node_status(
    nodegroup: &str,
    cluster: &str,
    region: &str,
    status: &str,
    min: u32,
    desired: u32,
    max: u32,
    ready: u32,
) {
    println!();
    println!("  {}", bold("Node Group Status"));
    println!("  Name:     {nodegroup}");
    println!("  Cluster:  {cluster}");
    println!("  Region:   {region}");
    println!("  Status:   {}", if status == "ACTIVE" { green(status) } else { yellow(status) });
    println!("  Scaling:  min={min} desired={desired} max={max}");
    println!("  Ready:    {} nodes in cluster", bold(&ready.to_string()));
}

// ---------------------------------------------------------------------------
// Verify output
// ---------------------------------------------------------------------------

/// Print infrastructure verification header.
pub fn print_verify_header() {
    print_phase("Infrastructure Verification");
}

/// Print a verification check line.
pub fn print_verify_check(component: &str, detail: &str, ok: bool) {
    let icon = if ok { green("OK") } else { red("!!") };
    let plain_label = format!("  {component}");
    let dots_needed = if plain_label.len() < 40 { 40 - plain_label.len() } else { 3 };
    let dots = dim(&".".repeat(dots_needed));
    println!("{plain_label} {dots} {detail} [{icon}]");
}

/// Print verification complete.
pub fn print_verify_complete() {
    println!();
    println!("  {}", bold_green("Infrastructure verified."));
}

// ---------------------------------------------------------------------------
// Cleanup / Reset output
// ---------------------------------------------------------------------------

/// Print reset-all header.
pub fn print_reset_header() {
    print_phase("Resetting Environment to Starting Conditions");
}

/// Print reset verification.
pub fn print_reset_verification(gw: &str, wh: &str, pods: u32, nodes: u32) {
    print_phase("Verifying Starting Conditions");
    println!("  Gateway:     {gw}");
    println!("  Webhook:     {wh}");
    println!("  Burst pods:  {pods}");
    println!("  Burst nodes: {nodes}");

    if pods == 0 && nodes == 0 {
        println!();
        println!("  {}", bold_green("Starting line confirmed. Ready for burst test."));
    } else {
        println!();
        print_warning(&format!("Not fully clean. Pods={pods}, Nodes={nodes}"));
    }
}

/// Print SIGINT cleanup header.
pub fn print_sigint_header() {
    eprintln!();
    eprintln!();
    eprintln!("{}", bold_red("SIGINT received -- running cleanup..."));
}

/// Print SIGINT cleanup step.
pub fn eprint_status(message: &str) {
    eprintln!("  {message}");
}

/// Print SIGINT cleanup warning.
pub fn eprint_warning(message: &str) {
    eprintln!("  {}", bold_yellow(&format!("WARNING: {message}")));
}

/// Print SIGINT cleanup complete.
pub fn eprint_complete() {
    eprintln!("  {}", bold_green("Cleanup complete -- exiting"));
}

// ---------------------------------------------------------------------------
// FluxCD output
// ---------------------------------------------------------------------------

/// Print flux wait header.
pub fn print_flux_header(count: usize, namespace: &str, timeout_secs: u64) {
    print_phase(&format!(
        "FluxCD Kustomizations: {count} in {namespace} (timeout: {timeout_secs}s)"
    ));
}

/// Print a kustomization becoming ready.
pub fn print_flux_ready(name: &str, elapsed_secs: u64) {
    let label = format!("  {name}");
    let dots_needed = if label.len() < 40 { 40 - label.len() } else { 3 };
    let dots = dim(&".".repeat(dots_needed));
    println!(
        "{label} {dots} {} ({})",
        bold_green("Ready"),
        dim(&format!("{elapsed_secs}s"))
    );
}

/// Print kustomization waiting status.
pub fn print_flux_waiting(name: &str) {
    println!("  {} {}", dim("..."), name);
}

/// Print all kustomizations ready.
pub fn print_flux_complete() {
    println!();
    println!("  {}", bold_green("All kustomizations ready."));
}

// ---------------------------------------------------------------------------
// Report / Confluence output
// ---------------------------------------------------------------------------

/// Print publish header.
pub fn print_publish_header() {
    print_phase("Publishing to Confluence");
}

/// Print publish result.
pub fn print_publish_result(url: &str) {
    println!("  Published: {}", cyan(url));
}

// ---------------------------------------------------------------------------
// Matrix cleanup output
// ---------------------------------------------------------------------------

/// Print matrix cleanup section.
pub fn print_matrix_cleanup(skip_scaling: bool) {
    print_phase("Matrix Cleanup");
    if !skip_scaling {
        print_action("Resetting deployments to 1 replica...");
        print_action("Resuming HelmReleases (FluxCD takes control again)...");
    }
}

/// Print node scale-down.
pub fn print_node_scaledown() {
    print_action("Scaling node group back to 0...");
}

/// Print matrix failure summary.
pub fn print_matrix_failures(failure_count: usize, total_count: usize, failures: &[String]) {
    println!();
    println!(
        "  {}",
        bold_red(&format!(
            "Matrix completed with {failure_count}/{total_count} scenario failures:"
        ))
    );
    for f in failures {
        println!("    {f}");
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format seconds as human-readable duration (e.g., "2m 34s", "45s", "1h 2m").
#[must_use]
pub fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{h}h")
        }
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        if s > 0 {
            format!("{m}m {s}s")
        } else {
            format!("{m}m")
        }
    } else {
        format!("{secs}s")
    }
}

/// Format milliseconds as a short human-readable string.
#[must_use]
pub fn format_ms(ms: u64) -> String {
    if ms >= 60_000 {
        let secs = ms / 1000;
        format_duration(secs)
    } else {
        #[allow(clippy::cast_precision_loss)]
        let secs = ms as f64 / 1000.0;
        format!("{secs:.1}s")
    }
}

/// Build a summary row for the results table.
#[must_use]
pub fn build_summary_row(
    name: &str,
    replicas: u32,
    running: Option<u32>,
    all_ready_ms: Option<u64>,
    injection_rate: Option<f64>,
    has_error: bool,
) -> SummaryRow {
    let pods = running.map_or_else(|| "-".to_string(), |r| format!("{r}/{replicas}"));
    let time = all_ready_ms.map_or_else(|| "-".to_string(), format_ms);
    let injection = injection_rate.map_or_else(|| "-".to_string(), |r| format!("{r:.1}%"));
    let status = if has_error { "FAIL".to_string() } else { "PASS".to_string() };

    SummaryRow {
        scenario: name.to_string(),
        pods,
        time,
        injection,
        status,
    }
}

/// Strip ANSI escape sequences and return the visual character count.
fn strip_ansi_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            len += 1;
        }
    }
    len
}

/// Build the inter-scenario cleanup header.
pub fn print_inter_scenario_cleanup() {
    println!();
    println!("  {}", bold("Inter-scenario cleanup"));
}

/// Print cooldown message.
pub fn print_cooldown(secs: u64) {
    print_action(&format!(
        "Drain complete -- cooling down {} before next scenario...",
        format_duration(secs)
    ));
}

/// Print the matrix results as a formatted summary.
pub fn print_matrix_summary(rows: &[SummaryRow]) {
    print_phase("Results Summary");
    print_summary_table(rows);
}

/// Write a formatted string to a buffer (for report generation, not stdout).
pub fn write_to(buf: &mut String, s: &str) {
    let _ = write!(buf, "{s}");
}

// ---------------------------------------------------------------------------
// Phase timing output
// ---------------------------------------------------------------------------

/// Print a phase timing line (single phase).
pub fn print_phase_timing(phase: &str, elapsed_ms: u64) {
    let secs = elapsed_ms / 1000;
    let label = format!("  {phase}");
    let plain_len = strip_ansi_len(&label);
    let dots_needed = if plain_len < DOT_LEADER_WIDTH { DOT_LEADER_WIDTH - plain_len } else { 3 };
    let dots = dim(&".".repeat(dots_needed));
    println!("{label} {dots} {}", bold(&format_duration(secs)));
}

/// Print the warmup sub-phase summary (Phase 2 breakdown).
pub fn print_warmup_summary(timings: &crate::types::WarmupTimings) {
    println!();
    println!("  {}", bold("Phase 2: WARMUP"));
    print_sub_phase_timing("2a. Nodes", timings.nodes_ms);
    print_sub_phase_timing("2b. Images", timings.images_ms);
    print_sub_phase_timing("2c. Gateway", timings.gateway_ms);
    print_sub_phase_timing("2d. Webhook", timings.webhook_ms);
    print_sub_phase_timing("2e. Gates", timings.gates_ms);
    let total_secs = timings.total_ms / 1000;
    println!(
        "  {} {}",
        bold("Total warmup:"),
        bold(&format_duration(total_secs))
    );
    println!();
}

/// Print a single sub-phase timing line.
fn print_sub_phase_timing(label: &str, elapsed_ms: u64) {
    let secs = elapsed_ms / 1000;
    let sub_label = format!("    {label}");
    let plain_len = strip_ansi_len(&sub_label);
    let dots_needed = if plain_len < DOT_LEADER_WIDTH { DOT_LEADER_WIDTH - plain_len } else { 3 };
    let dots = dim(&".".repeat(dots_needed));
    println!("{sub_label} {dots} {}", format_duration(secs));
}

/// Print the Phase 3 execution summary.
pub fn print_execution_summary(
    result: &crate::types::BurstResult,
    elapsed_ms: u64,
    gateway_replicas: u32,
) {
    println!();
    println!("  {}", bold("Phase 3: EXECUTION"));

    // Admission line
    let admission_time = result.time_to_full_admission_ms
        .map_or_else(|| "-".to_string(), |ms| format_ms(ms));
    println!(
        "    Admission: {}/{} in {} ({:.1} pods/sec)",
        result.pods_injected,
        result.replicas_requested,
        admission_time,
        result.admission_rate_pods_per_sec,
    );

    // Running line
    let running_time = result.time_to_all_ready_ms
        .map_or_else(
            || format!("{}/{} at timeout", result.pods_running, result.replicas_requested),
            |ms| format!(
                "{}/{} in {}",
                result.pods_running,
                result.replicas_requested,
                format_ms(ms),
            ),
        );
    println!("    Running: {running_time}");

    // Gateway throughput
    if gateway_replicas > 0 {
        println!(
            "    Gateway throughput: {:.1} pods/sec/replica",
            result.gateway_throughput_pods_per_sec
                / f64::from(gateway_replicas.max(1)),
        );
    }

    let total_secs = elapsed_ms / 1000;
    println!("    Total execution: {}", bold(&format_duration(total_secs)));
    println!();
}

/// Print the full phase timing breakdown at the end of a scenario.
pub fn print_phase_breakdown(timings: &crate::types::PhaseTimings) {
    let reset_secs = timings.reset_ms / 1000;
    let warmup_secs = timings.warmup_ms / 1000;
    let execution_secs = timings.execution_ms / 1000;
    let total = timings.reset_ms + timings.warmup_ms + timings.execution_ms;
    let total_secs = total / 1000;

    // Phase 1
    let label = "  Phase 1: RESET";
    let plain_len = strip_ansi_len(label);
    let dots_needed = if plain_len < DOT_LEADER_WIDTH { DOT_LEADER_WIDTH - plain_len } else { 3 };
    println!("{label} {} {}", dim(&".".repeat(dots_needed)), format_duration(reset_secs));

    // Phase 2
    let label = "  Phase 2: WARMUP";
    let plain_len = strip_ansi_len(label);
    let dots_needed = if plain_len < DOT_LEADER_WIDTH { DOT_LEADER_WIDTH - plain_len } else { 3 };
    println!("{label} {} {}", dim(&".".repeat(dots_needed)), format_duration(warmup_secs));

    // Sub-phases
    let detail = &timings.warmup_detail;
    print_sub_phase_timing("2a. Nodes", detail.nodes_ms);
    print_sub_phase_timing("2b. Images", detail.images_ms);
    print_sub_phase_timing("2c. Gateway", detail.gateway_ms);
    print_sub_phase_timing("2d. Webhook", detail.webhook_ms);
    print_sub_phase_timing("2e. Gates", detail.gates_ms);

    // Phase 3
    let label = "  Phase 3: EXECUTION";
    let plain_len = strip_ansi_len(label);
    let dots_needed = if plain_len < DOT_LEADER_WIDTH { DOT_LEADER_WIDTH - plain_len } else { 3 };
    println!("{label} {} {}", dim(&".".repeat(dots_needed)), format_duration(execution_secs));

    // Total
    println!();
    println!(
        "  {} {}",
        bold("Total scenario time:"),
        bold(&format_duration(total_secs))
    );
}
