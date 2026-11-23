//! Questions to the server's model: what a program or startup entry is, whether a leftover can go.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::rules::allowed_server;

/// The server's answer; `verdict` is one of safe, caution, keep, unknown and `confidence` one of high, medium, low.
#[derive(Serialize, Deserialize, Debug)]
pub struct Answer {
    pub summary: String,
    pub verdict: String,
    pub confidence: String,
    pub points: Vec<String>,
}

/// The server has no model configured.
pub const UNAVAILABLE: &str = "ai-unavailable";
/// Too many questions in a short time.
pub const BUSY: &str = "ai-busy";

/// Asks `server` about `facts` for one `feature` (program, startup, leftover), answered in `lang` (zh or en).
pub fn ask(server: &str, feature: &str, lang: &str, facts: Map<String, Value>) -> Result<Answer, String> {
    if !allowed_server(server) {
        return Err("the server address must start with https://".into());
    }
    let body = serde_json::json!({ "feature": feature, "lang": lang, "facts": facts }).to_string();
    let mut response = ureq::post(format!("{}/v1/ai/ask", server.trim_end_matches('/')))
        .header("content-type", "application/json")
        .config()
        // The model may think for a while; the server gives up on it after 90 seconds.
        .timeout_global(Some(Duration::from_secs(100)))
        .http_status_as_error(false)
        .build()
        .send(body)
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let text = response.body_mut().with_config().limit(256 * 1024).read_to_string().map_err(|e| e.to_string())?;
    match status {
        200 => serde_json::from_str(&text).map_err(|e| e.to_string()),
        503 => Err(UNAVAILABLE.into()),
        429 => Err(BUSY.into()),
        other => Err(format!("the server answered {other}")),
    }
}
