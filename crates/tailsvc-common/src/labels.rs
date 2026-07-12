use crate::hostname::parse_host_list;
use crate::{backend::parse_backend_url, Error, Result};
use std::collections::HashMap;

const PREFIX: &str = "tailsvc.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceLabels {
    pub hosts: Vec<crate::hostname::NormalizedHostname>,
    pub port: Option<u16>,
    pub backend: Option<crate::backend::Backend>,
    pub network: Option<String>,
}

pub fn labels_from_map(labels: &HashMap<String, String>) -> Option<ServiceLabels> {
    let enable = labels
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{PREFIX}enable")))?
        .1
        .as_str();
    if !enable.eq_ignore_ascii_case("true") {
        return None;
    }

    let hosts_raw = labels
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{PREFIX}hosts")))?
        .1;

    let hosts = parse_host_list(hosts_raw).ok()?;

    let port = labels
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{PREFIX}port")))
        .and_then(|(_, v)| parse_port(v).ok());

    let backend = labels
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{PREFIX}backend")))
        .and_then(|(_, v)| parse_backend_url(v).ok());

    let network = labels
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&format!("{PREFIX}network")))
        .map(|(_, v)| v.trim().to_string())
        .filter(|s| !s.is_empty());

    Some(ServiceLabels {
        hosts,
        port,
        backend,
        network,
    })
}

pub fn validate_service_labels(labels: &ServiceLabels) -> Result<()> {
    if labels.hosts.is_empty() {
        return Err(Error::LabelParse("no hosts".into()));
    }
    // Port may be omitted when backend is set, or when runtime can infer a single
    // exposed/published port (SPEC §6.3). Ambiguity is rejected at resolve time.
    let _ = labels.port;
    let _ = labels.backend;
    Ok(())
}

fn parse_port(s: &str) -> Result<u16> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| Error::LabelParse("invalid port".into()))?;
    if !(1..=65535).contains(&n) {
        return Err(Error::LabelParse("port out of range".into()));
    }
    Ok(n as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ignores_without_enable() {
        let m = map(&[("tailsvc.hosts", "x.internal")]);
        assert!(labels_from_map(&m).is_none());
    }

    #[test]
    fn parses_full() {
        let m = map(&[
            ("tailsvc.enable", "true"),
            ("tailsvc.hosts", "a.internal"),
            ("tailsvc.port", "3000"),
        ]);
        let s = labels_from_map(&m).unwrap();
        assert_eq!(s.port, Some(3000));
    }
}
