//! Small HTTP helpers built on `ureq`.

use std::time::Duration;

use crate::config;

/// Simple GET request; returns the body as text.
pub fn get_string(url: &str, timeout_secs: u64) -> Result<String, String> {
    let resp = ureq::get(url)
        .set("User-Agent", config::USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs))
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_string().map_err(|e| e.to_string())
}
