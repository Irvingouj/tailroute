use crate::{Error, Result};
use url::Url;

/// Validated HTTP backend target for v1 (`http://host:port` only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backend {
    pub host: String,
    pub port: u16,
}

impl Backend {
    pub fn authority(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}/", self.authority())
    }

    pub fn fingerprint(&self) -> String {
        format!("http://{}/", self.authority())
    }
}

pub fn parse_backend_url(raw: &str) -> Result<Backend> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidBackend("empty".into()));
    }
    if trimmed.as_bytes().iter().any(|&b| b < 32) {
        return Err(Error::InvalidBackend("control characters".into()));
    }

    let url = Url::parse(trimmed).map_err(|e| Error::InvalidBackend(e.to_string()))?;

    if url.scheme() != "http" {
        return Err(Error::InvalidBackend("only http scheme supported".into()));
    }
    if url.username() != "" || url.password().is_some() {
        return Err(Error::InvalidBackend("credentials not allowed".into()));
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(Error::InvalidBackend("path not allowed".into()));
    }
    if url.query().is_some() {
        return Err(Error::InvalidBackend("query not allowed".into()));
    }

    let host = url
        .host_str()
        .ok_or_else(|| Error::InvalidBackend("missing host".into()))?
        .to_string();

    let port = url.port().unwrap_or(80);
    if port == 0 {
        return Err(Error::InvalidBackend("invalid port".into()));
    }

    Ok(Backend { host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_host_port() {
        let b = parse_backend_url("http://172.18.0.4:3000").unwrap();
        assert_eq!(b.host, "172.18.0.4");
        assert_eq!(b.port, 3000);
    }

    #[test]
    fn rejects_https_and_creds() {
        assert!(parse_backend_url("https://x:1").is_err());
        assert!(parse_backend_url("http://user@x:1").is_err());
    }
}
