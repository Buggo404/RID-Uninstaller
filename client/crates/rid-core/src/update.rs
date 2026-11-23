//! Asks the server whether a newer version has been published.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::rules::allowed_server;

#[derive(Serialize, Deserialize, Debug)]
pub struct Release {
    pub version: String,
    /// Where to get it. Opened in the browser; nothing is downloaded or run from here.
    pub url: String,
    #[serde(default)]
    pub notes: String,
}

fn parts(version: &str) -> Option<[u32; 3]> {
    let mut numbers = version.trim().split('.').map(|n| n.parse::<u32>().ok());
    let parsed = [numbers.next()??, numbers.next()??, numbers.next()??];
    numbers.next().is_none().then_some(parsed)
}

/// Whether `candidate` is a later version than `current`. Anything unreadable is not newer.
pub fn newer(current: &str, candidate: &str) -> bool {
    matches!((parts(current), parts(candidate)), (Some(have), Some(offered)) if offered > have)
}

/// The release on `server` if it is newer than `current`; `Ok(None)` when up to date.
pub fn check(server: &str, current: &str) -> Result<Option<Release>, String> {
    if !allowed_server(server) {
        return Err("the server address must start with https://".into());
    }
    let mut response = ureq::get(format!("{}/v1/update", server.trim_end_matches('/')))
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        other => return Err(format!("the server answered {other}")),
    }
    let body = response.body_mut().with_config().limit(64 * 1024).read_to_string().map_err(|e| e.to_string())?;
    let release: Release = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    // The address ends up in the user's browser, so it is held to the same rule as the server's own.
    Ok((newer(current, &release.version) && allowed_server(&release.url)).then_some(release))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(newer("0.1.0", "0.1.1"));
        assert!(newer("0.9.0", "0.10.0"));
        assert!(newer("0.9.9", "1.0.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("1.0.0", "0.9.9"));
        for odd in ["", "1.2", "1.2.3.4", "v1.2.3", "1.2.x", "1.2.3-beta"] {
            assert!(!newer("0.1.0", odd), "{odd}");
        }
    }
}
