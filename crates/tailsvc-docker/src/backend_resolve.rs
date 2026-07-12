use crate::types::DiscoveredService;
use tailsvc_common::backend::Backend;
use tailsvc_common::{Error, Result};

/// Backend resolution priority (SPEC §7):
/// 1. Explicit tailsvc.backend
/// 2. Published host port
/// 3. network_mode=host
/// 4. Container network IP
/// 5. Reject as ambiguous or unreachable
pub fn resolve_backend(svc: &DiscoveredService) -> Result<Backend> {
    if let Some(ref b) = svc.labels.backend {
        return Ok(b.clone());
    }

    let port = svc
        .labels
        .port
        .or_else(|| single_exposed_port(svc))
        .or_else(|| single_published_container_port(svc))
        .ok_or_else(|| Error::AmbiguousBackend("no port specified".into()))?;

    // 2. Published host port for the chosen container port
    if let Some(pubp) = select_published_port(svc, port)? {
        let host = if pubp.host_ip == "0.0.0.0" || pubp.host_ip.is_empty() {
            "127.0.0.1".into()
        } else {
            pubp.host_ip.clone()
        };
        return Ok(Backend {
            host,
            port: pubp.host_port,
        });
    }

    // 3. Host network
    if svc.network_mode.as_deref() == Some("host") {
        return Ok(Backend {
            host: "127.0.0.1".into(),
            port,
        });
    }

    // 4. Container network IP
    let ip = select_container_ip(svc)?;
    Ok(Backend { host: ip, port })
}

fn single_exposed_port(svc: &DiscoveredService) -> Option<u16> {
    if svc.exposed_ports.len() == 1 {
        Some(svc.exposed_ports[0])
    } else {
        None
    }
}

fn single_published_container_port(svc: &DiscoveredService) -> Option<u16> {
    let mut ports: Vec<u16> = svc
        .published_ports
        .iter()
        .map(|p| p.container_port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    if ports.len() == 1 {
        Some(ports[0])
    } else {
        None
    }
}

fn select_published_port(
    svc: &DiscoveredService,
    container_port: u16,
) -> Result<Option<crate::types::PublishedPort>> {
    let matches: Vec<_> = svc
        .published_ports
        .iter()
        .filter(|p| p.container_port == container_port)
        .cloned()
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].clone())),
        _ => {
            let local: Vec<_> = matches
                .iter()
                .filter(|p| {
                    p.host_ip == "127.0.0.1" || p.host_ip == "0.0.0.0" || p.host_ip.is_empty()
                })
                .collect();
            if local.len() == 1 {
                Ok(Some(local[0].clone()))
            } else {
                Err(Error::AmbiguousBackend(
                    "multiple published bindings for port".into(),
                ))
            }
        }
    }
}

fn select_container_ip(svc: &DiscoveredService) -> Result<String> {
    if let Some(ref net) = svc.labels.network {
        return svc
            .networks
            .get(net)
            .cloned()
            .ok_or_else(|| Error::AmbiguousBackend(format!("network {net} not found")));
    }
    let usable: Vec<_> = svc.networks.values().cloned().collect();
    match usable.len() {
        0 => Err(Error::AmbiguousBackend("no container IP".into())),
        1 => Ok(usable[0].clone()),
        _ => Err(Error::AmbiguousBackend(
            "multiple networks; set tailsvc.network".into(),
        )),
    }
}

pub fn backend_url(backend: &Backend) -> String {
    format!("http://{}/", backend.authority())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PublishedPort;
    use tailsvc_common::hostname::normalize_hostname;
    use tailsvc_common::labels::ServiceLabels;

    fn labels(port: Option<u16>, backend: Option<Backend>, network: Option<&str>) -> ServiceLabels {
        ServiceLabels {
            hosts: vec![normalize_hostname("x.internal").unwrap()],
            port,
            backend,
            network: network.map(|s| s.into()),
        }
    }

    fn svc(
        labels: ServiceLabels,
        nets: &[(&str, &str)],
        network_mode: Option<&str>,
        published: Vec<PublishedPort>,
        exposed: Vec<u16>,
    ) -> DiscoveredService {
        DiscoveredService {
            container_id: "c1".into(),
            container_name: "n".into(),
            labels,
            network_mode: network_mode.map(|s| s.into()),
            published_ports: published,
            networks: nets
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            exposed_ports: exposed,
        }
    }

    #[test]
    fn explicit_backend_wins() {
        let b = Backend {
            host: "192.168.1.50".into(),
            port: 5000,
        };
        let s = svc(
            labels(Some(3000), Some(b.clone()), None),
            &[("br", "1.2.3.4")],
            None,
            vec![],
            vec![],
        );
        assert_eq!(resolve_backend(&s).unwrap(), b);
    }

    #[test]
    fn published_port_before_host_network() {
        let s = svc(
            labels(Some(3000), None, None),
            &[],
            Some("host"),
            vec![PublishedPort {
                container_port: 3000,
                host_ip: "127.0.0.1".into(),
                host_port: 8080,
            }],
            vec![],
        );
        let b = resolve_backend(&s).unwrap();
        assert_eq!(b.host, "127.0.0.1");
        assert_eq!(b.port, 8080);
    }

    #[test]
    fn host_network_default() {
        let s = svc(
            labels(Some(8080), None, None),
            &[],
            Some("host"),
            vec![],
            vec![],
        );
        let b = resolve_backend(&s).unwrap();
        assert_eq!(b.host, "127.0.0.1");
        assert_eq!(b.port, 8080);
    }

    #[test]
    fn published_0_0_0_0_maps_to_loopback() {
        let s = svc(
            labels(Some(80), None, None),
            &[],
            None,
            vec![PublishedPort {
                container_port: 80,
                host_ip: "0.0.0.0".into(),
                host_port: 8080,
            }],
            vec![],
        );
        let b = resolve_backend(&s).unwrap();
        assert_eq!(b.host, "127.0.0.1");
        assert_eq!(b.port, 8080);
    }

    #[test]
    fn bridge_single_network() {
        let s = svc(
            labels(Some(3000), None, None),
            &[("frontend", "172.18.0.4")],
            None,
            vec![],
            vec![],
        );
        let b = resolve_backend(&s).unwrap();
        assert_eq!(b.host, "172.18.0.4");
        assert_eq!(b.port, 3000);
    }

    #[test]
    fn multi_network_requires_selection() {
        let s = svc(
            labels(Some(80), None, None),
            &[("a", "1.1.1.1"), ("b", "2.2.2.2")],
            None,
            vec![],
            vec![],
        );
        assert!(resolve_backend(&s).is_err());
    }

    #[test]
    fn multi_network_with_label() {
        let s = svc(
            labels(Some(80), None, Some("b")),
            &[("a", "1.1.1.1"), ("b", "2.2.2.2")],
            None,
            vec![],
            vec![],
        );
        assert_eq!(resolve_backend(&s).unwrap().host, "2.2.2.2");
    }

    #[test]
    fn infer_port_from_single_exposed() {
        let s = svc(
            labels(None, None, None),
            &[("br", "172.18.0.9")],
            None,
            vec![],
            vec![3000],
        );
        let b = resolve_backend(&s).unwrap();
        assert_eq!(b.port, 3000);
        assert_eq!(b.host, "172.18.0.9");
    }

    #[test]
    fn ambiguous_exposed_ports_fail() {
        let s = svc(
            labels(None, None, None),
            &[("br", "172.18.0.9")],
            None,
            vec![],
            vec![3000, 3001],
        );
        assert!(resolve_backend(&s).is_err());
    }
}
