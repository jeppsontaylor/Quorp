pub fn default_local_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

pub fn validate_local_runtime_base_url(base_url: &str) -> Result<url::Url, String> {
    let normalized = base_url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(normalized)
        .map_err(|error| format!("Invalid SSD-MOE base URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(
                "SSD-MOE base URL must use http:// or https:// for local managed runtime access."
                    .to_string(),
            );
        }
    }
    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    };
    if parsed.scheme() == "http" && !is_loopback {
        return Err("HTTP SSD-MOE base URL must stay on localhost or a loopback IP.".to_string());
    }
    if parsed.host().is_none() {
        return Err("SSD-MOE base URL must include a host.".to_string());
    }
    Ok(parsed)
}

pub fn local_bearer_token(base_url: &str) -> Result<String, String> {
    validate_local_runtime_base_url(base_url)?;
    Ok("local".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_validation_allows_local_hosts() {
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:8080/v1",
            "http://[::1]:8080/v1",
            "https://warpos-capture-probe:8443/quorp/v1",
        ] {
            validate_local_runtime_base_url(url).expect("local runtime URL should pass");
        }
    }

    #[test]
    fn loopback_validation_rejects_remote_hosts() {
        let error =
            validate_local_runtime_base_url("http://example.com/v1").expect_err("remote host");
        assert!(error.contains("loopback"));
    }
}
