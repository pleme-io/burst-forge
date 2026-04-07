//! Report generation and Confluence publishing.

use std::fmt::Write;
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::{ConfluenceConfig, Config};
use crate::types::MatrixReport;

/// Generate a Confluence storage-format (XHTML) report from matrix results.
#[must_use]
pub fn generate_report(report: &MatrixReport, config: &Config) -> (String, String) {
    let title = format!("Burst Test Report {}", report.timestamp);

    let total = report.scenarios.len();
    let passed = report.scenarios.iter().filter(|s| s.error.is_none()).count();
    let failed = total - passed;
    let status = if failed == 0 { "PASS" } else { "FAIL" };

    let mut html = String::new();

    // Executive summary
    html.push_str("<h2>Executive Summary</h2>");
    let _ = write!(
        html,
        "<p><strong>Status:</strong> {status} | \
         <strong>Scenarios:</strong> {total} | \
         <strong>Passed:</strong> {passed} | \
         <strong>Failed:</strong> {failed}</p>"
    );

    // Per-scenario table
    html.push_str("<h2>Scenario Results</h2>");
    html.push_str(
        "<table><thead><tr>\
         <th>Scenario</th>\
         <th>Replicas</th>\
         <th>Pods Running</th>\
         <th>Peak Running</th>\
         <th>Secrets</th>\
         <th>Injection Rate</th>\
         <th>First Ready (ms)</th>\
         <th>All Ready (ms)</th>\
         <th>Error</th>\
         </tr></thead><tbody>",
    );

    for s in &report.scenarios {
        let pods = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| b.pods_running.to_string());
        let peak = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| b.peak_running.to_string());
        let secrets = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| b.total_secrets_injected.to_string());
        let rate = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| format!("{:.1}%", b.injection_success_rate));
        let first = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| b.time_to_first_ready_ms.to_string());
        let all = s.burst.as_ref().and_then(|b| b.time_to_all_ready_ms).map_or_else(
            || "-".to_string(),
            |v| v.to_string(),
        );
        let error = s.error.as_deref().unwrap_or("-");

        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{pods}</td><td>{peak}</td><td>{secrets}</td><td>{rate}</td>\
             <td>{first}</td><td>{all}</td><td>{error}</td></tr>",
            s.name, s.replicas
        );
    }

    html.push_str("</tbody></table>");

    // Phase Timings
    html.push_str("<h2>Phase Timings</h2>");
    html.push_str(
        "<table><thead><tr>\
         <th>Scenario</th>\
         <th>Phase 1: RESET</th>\
         <th>Phase 2: WARMUP</th>\
         <th>2a. Nodes</th>\
         <th>2b. Images</th>\
         <th>2c. Gateway</th>\
         <th>2d. Webhook</th>\
         <th>2e. Gates</th>\
         <th>Phase 3: EXECUTION</th>\
         </tr></thead><tbody>",
    );

    for s in &report.scenarios {
        if let Some(t) = &s.phase_timings {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{:.1}s</td><td>{:.1}s</td>\
                 <td>{:.1}s</td><td>{:.1}s</td><td>{:.1}s</td>\
                 <td>{:.1}s</td><td>{:.1}s</td><td>{:.1}s</td></tr>",
                s.name,
                t.reset_ms as f64 / 1000.0,
                t.warmup_ms as f64 / 1000.0,
                t.warmup_detail.nodes_ms as f64 / 1000.0,
                t.warmup_detail.images_ms as f64 / 1000.0,
                t.warmup_detail.gateway_ms as f64 / 1000.0,
                t.warmup_detail.webhook_ms as f64 / 1000.0,
                t.warmup_detail.gates_ms as f64 / 1000.0,
                t.execution_ms as f64 / 1000.0,
            );
        } else {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>-</td><td>-</td><td>-</td><td>-</td>\
                 <td>-</td><td>-</td><td>-</td><td>-</td></tr>",
                s.name
            );
        }
    }

    html.push_str("</tbody></table>");

    // Execution Metrics
    html.push_str("<h2>Execution Metrics</h2>");
    html.push_str(
        "<table><thead><tr>\
         <th>Scenario</th>\
         <th>Admission Rate (pods/sec)</th>\
         <th>Gateway Throughput (pods/sec)</th>\
         <th>Time to 50% Running</th>\
         <th>Time to Full Admission</th>\
         </tr></thead><tbody>",
    );

    for s in &report.scenarios {
        if let Some(b) = &s.burst {
            let t50 = b.time_to_50pct_running_ms.map_or_else(
                || "-".to_string(),
                |ms| format!("{:.1}s", ms as f64 / 1000.0),
            );
            let tadm = b.time_to_full_admission_ms.map_or_else(
                || "-".to_string(),
                |ms| format!("{:.1}s", ms as f64 / 1000.0),
            );
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{:.1}</td><td>{:.1}</td><td>{t50}</td><td>{tadm}</td></tr>",
                s.name,
                b.admission_rate_pods_per_sec,
                b.gateway_throughput_pods_per_sec,
            );
        } else {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>-</td><td>-</td><td>-</td><td>-</td></tr>",
                s.name
            );
        }
    }

    html.push_str("</tbody></table>");

    // Infrastructure details
    html.push_str("<h2>Infrastructure</h2>");
    html.push_str("<table><thead><tr>\
         <th>Scenario</th>\
         <th>Nodes</th>\
         <th>Gateway Replicas</th>\
         <th>Webhook Replicas</th>\
         </tr></thead><tbody>");

    for s in &report.scenarios {
        let nodes = s
            .burst
            .as_ref()
            .map_or_else(|| "-".to_string(), |b| b.nodes.to_string());
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{nodes}</td><td>{}</td><td>{}</td></tr>",
            s.name, s.gateway_replicas, s.webhook_replicas
        );
    }

    html.push_str("</tbody></table>");

    // Configuration
    html.push_str("<h2>Configuration</h2>");
    let _ = write!(
        html,
        "<ul>\
         <li><strong>Namespace:</strong> {}</li>\
         <li><strong>Deployment:</strong> {}</li>\
         <li><strong>Injection Mode:</strong> {:?}</li>\
         </ul>",
        config.namespace, config.deployment, config.injection_mode
    );

    // Raw JSON
    html.push_str("<h2>Raw Data</h2>");
    let json = serde_json::to_string_pretty(report).unwrap_or_default();
    let _ = write!(
        html,
        "<ac:structured-macro ac:name=\"code\">\
         <ac:parameter ac:name=\"language\">json</ac:parameter>\
         <ac:plain-text-body><![CDATA[{json}]]></ac:plain-text-body>\
         </ac:structured-macro>"
    );

    (title, html)
}

/// Publish a page to Confluence as a child of the configured parent page.
///
/// Uses the Confluence REST API v1 via `curl` (subprocess pattern consistent
/// with how burst-forge uses `kubectl` and `aws`).
///
/// The API token is read from the `CONFLUENCE_API_TOKEN` environment variable.
///
/// # Errors
///
/// Returns an error if the token is missing, or the API call fails.
pub fn publish_to_confluence(
    conf: &ConfluenceConfig,
    title: &str,
    content: &str,
) -> anyhow::Result<String> {
    // Token discovery: env var -> configured token_path (with ~ expansion)
    let expanded_path = if conf.token_path.starts_with("~/") {
        dirs::home_dir()
            .map(|h| h.join(&conf.token_path[2..]).to_string_lossy().to_string())
            .unwrap_or_else(|| conf.token_path.clone())
    } else {
        conf.token_path.clone()
    };
    let token = std::env::var("CONFLUENCE_API_TOKEN").ok().or_else(|| {
        std::fs::read_to_string(&expanded_path).ok().map(|s| s.trim().to_string())
    }).with_context(|| format!(
        "Confluence API token not found. Set CONFLUENCE_API_TOKEN env var or ensure {} exists",
        expanded_path
    ))?;

    // Build Basic auth header: base64(email:token)
    let credentials = format!("{}:{token}", conf.user_email);
    let encoded = base64_encode(&credentials);
    let auth_header = format!("Basic {encoded}");

    // Build the JSON payload for Confluence REST API v1
    let payload = serde_json::json!({
        "type": "page",
        "title": title,
        "ancestors": [{"id": conf.parent_page_id}],
        "space": {"key": conf.space_key},
        "body": {
            "storage": {
                "value": content,
                "representation": "storage"
            }
        }
    });

    let url = format!("https://{}/wiki/rest/api/content", conf.base_url);

    let output = Command::new("curl")
        .args([
            "-s",
            "-w",
            "\n%{http_code}",
            "-X",
            "POST",
            &url,
            "-H",
            &format!("Authorization: {auth_header}"),
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload.to_string(),
        ])
        .output()
        .context("Failed to execute curl")?;

    let raw = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = raw.trim().rsplitn(2, '\n').collect();

    let (status_str, body) = if lines.len() == 2 {
        (lines[0], lines[1])
    } else {
        (lines.first().copied().unwrap_or(""), "")
    };

    let status: u16 = status_str.parse().unwrap_or(0);

    if !(200..300).contains(&status) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Confluence API returned HTTP {status}\nBody: {body}\nStderr: {stderr}"
        );
    }

    // Extract the page URL from the response
    let resp: serde_json::Value = serde_json::from_str(body)
        .context("Failed to parse Confluence response JSON")?;

    let page_url = resp["_links"]["base"]
        .as_str()
        .zip(resp["_links"]["webui"].as_str())
        .map_or_else(
            || format!("https://{}/wiki", conf.base_url),
            |(base, webui)| format!("{base}{webui}"),
        );

    Ok(page_url)
}

/// Simple base64 encoder (avoids adding a dependency just for this).
fn base64_encode(input: &str) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

/// Export matrix results to JSON file in the configured output directory.
///
/// Writes `results-{sanitized-timestamp}.json` with the full `MatrixReport`.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or the file cannot be written.
pub fn export_json(report: &MatrixReport, output_dir: &str) -> anyhow::Result<String> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory: {output_dir}"))?;

    let safe_ts = report.timestamp.replace(':', "-").replace('+', "p");
    let filename = format!("results-{safe_ts}.json");
    let path = std::path::Path::new(output_dir).join(&filename);

    let json = serde_json::to_string_pretty(report)
        .context("Failed to serialize MatrixReport")?;
    std::fs::write(&path, &json)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BurstResult, MatrixReport, PhaseTimings, ScenarioResult, WarmupTimings};

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode("hello"), "aGVsbG8=");
        assert_eq!(base64_encode("user:token"), "dXNlcjp0b2tlbg==");
        assert_eq!(base64_encode(""), "");
    }

    #[test]
    fn base64_encode_single_char() {
        assert_eq!(base64_encode("a"), "YQ==");
    }

    #[test]
    fn base64_encode_two_chars() {
        assert_eq!(base64_encode("ab"), "YWI=");
    }

    #[test]
    fn base64_encode_three_chars() {
        assert_eq!(base64_encode("abc"), "YWJj");
    }

    #[test]
    fn base64_encode_special_chars() {
        assert_eq!(base64_encode("user@example.com:s3cr3t!"), "dXNlckBleGFtcGxlLmNvbTpzM2NyM3Qh");
    }

    fn make_burst_result(running: u32, replicas: u32) -> BurstResult {
        BurstResult {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            replicas_requested: replicas,
            pods_running: running,
            pods_failed: 0,
            pods_pending: 0,
            pods_injected: running,
            injection_success_rate: 100.0,
            time_to_first_ready_ms: 500,
            time_to_all_ready_ms: Some(5000),
            time_to_full_admission_ms: Some(4000),
            time_to_50pct_running_ms: Some(2500),
            admission_rate_pods_per_sec: 20.0,
            gateway_throughput_pods_per_sec: 18.0,
            duration_ms: 6000,
            nodes: 3,
            iteration: 1,
            total_secrets_injected: running * 2,
            peak_running: running,
            prediction: None,
        }
    }

    fn make_report(scenarios: Vec<ScenarioResult>) -> MatrixReport {
        MatrixReport {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            scenarios,
        }
    }

    fn default_config() -> Config {
        serde_json::from_str("{}").unwrap()
    }

    #[test]
    fn generate_report_all_pass() {
        let report = make_report(vec![
            ScenarioResult {
                name: "test-100".to_string(),
                replicas: 100,
                gateway_replicas: 5,
                webhook_replicas: 3,
                verify: None,
                burst: Some(make_burst_result(100, 100)),
                phase_timings: None,
                error: None,
            },
        ]);
        let config = default_config();
        let (title, html) = generate_report(&report, &config);

        assert!(title.contains("Burst Test Report"));
        assert!(html.contains("PASS"));
        assert!(!html.contains("FAIL"));
        assert!(html.contains("test-100"));
        assert!(html.contains("100.0%"));
    }

    #[test]
    fn generate_report_with_failure() {
        let report = make_report(vec![
            ScenarioResult {
                name: "fail-scenario".to_string(),
                replicas: 500,
                gateway_replicas: 3,
                webhook_replicas: 3,
                verify: None,
                burst: None,
                phase_timings: None,
                error: Some("Gate 3 FAILED: infrastructure not ready".to_string()),
            },
        ]);
        let config = default_config();
        let (_, html) = generate_report(&report, &config);

        assert!(html.contains("FAIL"));
        assert!(html.contains("fail-scenario"));
        assert!(html.contains("Gate 3 FAILED"));
    }

    #[test]
    fn generate_report_with_phase_timings() {
        let report = make_report(vec![
            ScenarioResult {
                name: "timed".to_string(),
                replicas: 100,
                gateway_replicas: 5,
                webhook_replicas: 3,
                verify: None,
                burst: Some(make_burst_result(100, 100)),
                phase_timings: Some(PhaseTimings {
                    reset_ms: 5000,
                    warmup_ms: 120_000,
                    warmup_detail: WarmupTimings {
                        nodes_ms: 60_000,
                        images_ms: 30_000,
                        ipamd_warmup_ms: 0,
                        gateway_ms: 15_000,
                        webhook_ms: 10_000,
                        gates_ms: 5000,
                        patches_ms: 0,
                        total_ms: 120_000,
                    },
                    execution_ms: 8000,
                }),
                error: None,
            },
        ]);
        let config = default_config();
        let (_, html) = generate_report(&report, &config);

        assert!(html.contains("Phase Timings"));
        assert!(html.contains("5.0s")); // reset
        assert!(html.contains("120.0s")); // warmup
    }

    #[test]
    fn generate_report_without_burst_shows_dashes() {
        let report = make_report(vec![
            ScenarioResult {
                name: "no-burst".to_string(),
                replicas: 100,
                gateway_replicas: 1,
                webhook_replicas: 1,
                verify: None,
                burst: None,
                phase_timings: None,
                error: None,
            },
        ]);
        let config = default_config();
        let (_, html) = generate_report(&report, &config);

        assert!(html.contains("<td>-</td>"));
    }

    #[test]
    fn generate_report_includes_config_details() {
        let config = default_config();
        let report = make_report(vec![]);
        let (_, html) = generate_report(&report, &config);

        assert!(html.contains("scale-test"));
        assert!(html.contains("nginx-burst"));
        assert!(html.contains("Env"));
    }

    #[test]
    fn generate_report_includes_raw_json() {
        let report = make_report(vec![]);
        let config = default_config();
        let (_, html) = generate_report(&report, &config);

        assert!(html.contains("CDATA"));
        assert!(html.contains("Raw Data"));
    }

    #[test]
    fn export_json_creates_file() {
        let dir = std::env::temp_dir().join("burst-forge-test-export");
        let _ = std::fs::remove_dir_all(&dir);

        let report = make_report(vec![]);
        let path = export_json(&report, &dir.to_string_lossy()).unwrap();

        assert!(std::path::Path::new(&path).exists());
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: MatrixReport = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.timestamp, "2024-01-01T00:00:00Z");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_json_sanitizes_timestamp() {
        let dir = std::env::temp_dir().join("burst-forge-test-sanitize");
        let _ = std::fs::remove_dir_all(&dir);

        let report = MatrixReport {
            timestamp: "2024-01-01T12:30:45+05:30".to_string(),
            scenarios: vec![],
        };
        let path = export_json(&report, &dir.to_string_lossy()).unwrap();

        assert!(path.contains("12-30-45p05-30"));
        assert!(!path.contains(':'));
        assert!(!path.contains('+'));

        std::fs::remove_dir_all(&dir).ok();
    }
}
