//! Report generation and Confluence publishing.

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
    html.push_str(&format!(
        "<p><strong>Status:</strong> {status} | \
         <strong>Scenarios:</strong> {total} | \
         <strong>Passed:</strong> {passed} | \
         <strong>Failed:</strong> {failed}</p>"
    ));

    // Per-scenario table
    html.push_str("<h2>Scenario Results</h2>");
    html.push_str(
        "<table><thead><tr>\
         <th>Scenario</th>\
         <th>Replicas</th>\
         <th>Pods Running</th>\
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

        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{pods}</td><td>{rate}</td>\
             <td>{first}</td><td>{all}</td><td>{error}</td></tr>",
            s.name, s.replicas
        ));
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
        html.push_str(&format!(
            "<tr><td>{}</td><td>{nodes}</td><td>{}</td><td>{}</td></tr>",
            s.name, s.gateway_replicas, s.webhook_replicas
        ));
    }

    html.push_str("</tbody></table>");

    // Configuration
    html.push_str("<h2>Configuration</h2>");
    html.push_str(&format!(
        "<ul>\
         <li><strong>Namespace:</strong> {}</li>\
         <li><strong>Deployment:</strong> {}</li>\
         <li><strong>Injection Mode:</strong> {:?}</li>\
         </ul>",
        config.namespace, config.deployment, config.injection_mode
    ));

    // Raw JSON
    html.push_str("<h2>Raw Data</h2>");
    let json = serde_json::to_string_pretty(report).unwrap_or_default();
    let escaped = json
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    html.push_str(&format!(
        "<ac:structured-macro ac:name=\"code\">\
         <ac:parameter ac:name=\"language\">json</ac:parameter>\
         <ac:plain-text-body><![CDATA[{json}]]></ac:plain-text-body>\
         </ac:structured-macro>"
    ));
    // Also include a plain <pre> fallback in case the macro is not supported
    let _ = escaped; // used below only if we switch to plain pre

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
    let token = std::env::var("CONFLUENCE_API_TOKEN")
        .context("CONFLUENCE_API_TOKEN env var is required for Confluence publishing")?;

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

    if status != 200 {
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
        .map(|(base, webui)| format!("{base}{webui}"))
        .unwrap_or_else(|| format!("https://{}/wiki", conf.base_url));

    Ok(page_url)
}

/// Simple base64 encoder (avoids adding a dependency just for this).
fn base64_encode(input: &str) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut result = String::with_capacity((bytes.len() + 2) / 3 * 4);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode("hello"), "aGVsbG8=");
        assert_eq!(base64_encode("user:token"), "dXNlcjp0b2tlbg==");
        assert_eq!(base64_encode(""), "");
    }
}
