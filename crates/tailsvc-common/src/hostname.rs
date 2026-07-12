use crate::{Error, Result};
use std::fmt;

/// Normalized hostname (lowercase, IDNA punycode for internationalized labels).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NormalizedHostname(String);

impl NormalizedHostname {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for NormalizedHostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Normalize and validate a hostname for registration/DNS.
pub fn normalize_hostname(input: &str) -> Result<NormalizedHostname> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidHostname("empty".into()));
    }
    if trimmed.contains(':') {
        return Err(Error::InvalidHostname(
            "port not allowed in hostname".into(),
        ));
    }
    if trimmed.contains('/') {
        return Err(Error::InvalidHostname(
            "path not allowed in hostname".into(),
        ));
    }
    if trimmed.contains("://") {
        return Err(Error::InvalidHostname(
            "scheme not allowed in hostname".into(),
        ));
    }

    let ascii =
        idna::domain_to_ascii(trimmed).map_err(|e| Error::InvalidHostname(format!("idna: {e}")))?;

    let lower = ascii.to_ascii_lowercase();
    validate_hostname_syntax(&lower)?;
    Ok(NormalizedHostname(lower))
}

fn validate_hostname_syntax(host: &str) -> Result<()> {
    if host.len() > 253 {
        return Err(Error::InvalidHostname("too long".into()));
    }
    if host.ends_with('.') {
        return Err(Error::InvalidHostname("trailing dot not allowed".into()));
    }
    for label in host.split('.') {
        if label.is_empty() {
            return Err(Error::InvalidHostname("empty label".into()));
        }
        if label.len() > 63 {
            return Err(Error::InvalidHostname("label too long".into()));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(Error::InvalidHostname("label hyphen placement".into()));
        }
        for c in label.chars() {
            if !c.is_ascii_alphanumeric() && c != '-' {
                return Err(Error::InvalidHostname(format!("invalid char: {c}")));
            }
        }
    }
    Ok(())
}

/// Parse comma-separated host list; dedupe; reject on any invalid entry.
pub fn parse_host_list(raw: &str) -> Result<Vec<NormalizedHostname>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for part in raw.split(',') {
        let h = normalize_hostname(part)?;
        if seen.insert(h.as_str().to_string()) {
            out.push(h);
        }
    }
    if out.is_empty() {
        return Err(Error::InvalidHostname(
            "at least one hostname required".into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_and_trim() {
        let h = normalize_hostname("  Grafana.Internal  ").unwrap();
        assert_eq!(h.as_str(), "grafana.internal");
    }

    #[test]
    fn rejects_port_and_path() {
        assert!(normalize_hostname("foo:80").is_err());
        assert!(normalize_hostname("foo/bar").is_err());
        assert!(normalize_hostname("http://x").is_err());
    }

    #[test]
    fn parse_dedupes() {
        let v = parse_host_list("a.internal, b.internal ,a.internal").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].as_str(), "a.internal");
        assert_eq!(v[1].as_str(), "b.internal");
    }
}
