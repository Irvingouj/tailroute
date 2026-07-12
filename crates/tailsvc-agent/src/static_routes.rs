//! Merge config `[[static_routes]]` into the agent desired route set.
//!
//! Precedence: **static overrides Docker** on the same hostname (logged by caller).

use crate::config::StaticRouteConfig;
use tailsvc_common::api::RouteEntry;
use tailsvc_common::backend::{parse_backend_url, Backend};
use tailsvc_common::hostname::normalize_hostname;
use tailsvc_proxy::BackendRoute;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct PreparedStaticRoute {
    pub hosts: Vec<String>,
    pub backend: Backend,
    pub backend_url: String,
}

/// Validate config entries; skip invalid ones with warnings (agent keeps running).
pub fn prepare_static_routes(entries: &[StaticRouteConfig]) -> Vec<PreparedStaticRoute> {
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let backend = match parse_backend_url(&e.backend) {
            Ok(b) => b,
            Err(err) => {
                warn!(index = i, backend = %e.backend, error = %err, "static_routes: skip bad backend");
                continue;
            }
        };
        let mut hosts = Vec::new();
        for h in &e.hosts {
            match normalize_hostname(h) {
                Ok(n) => hosts.push(n.into_inner()),
                Err(err) => {
                    warn!(index = i, host = %h, error = %err, "static_routes: skip bad host");
                }
            }
        }
        if hosts.is_empty() {
            warn!(index = i, "static_routes: skip entry with no valid hosts");
            continue;
        }
        let backend_url = backend.base_url();
        out.push(PreparedStaticRoute {
            hosts,
            backend,
            backend_url,
        });
    }
    out
}

/// Apply static routes onto maps built from Docker (or empty).
/// Returns number of hostnames written; `overridden` counts Docker hosts replaced.
pub fn apply_static_routes(
    local: &mut std::collections::HashMap<String, BackendRoute>,
    api_routes: &mut Vec<RouteEntry>,
    hb_refs: &mut Vec<tailsvc_common::api::HeartbeatRouteRef>,
    static_routes: &[PreparedStaticRoute],
) -> (usize, usize) {
    let mut written = 0usize;
    let mut overridden = 0usize;
    for sr in static_routes {
        for host in &sr.hosts {
            if local.contains_key(host) {
                overridden += 1;
                warn!(
                    hostname = %host,
                    "static_routes override existing docker route"
                );
                // Remove prior API/hb entries for this host so PUT set is consistent.
                api_routes.retain(|r| r.hostname != *host);
                hb_refs.retain(|r| r.hostname != *host);
            }
            local.insert(
                host.clone(),
                BackendRoute {
                    backend: sr.backend.clone(),
                    container_id: None,
                    container_name: Some("static".into()),
                },
            );
            api_routes.push(RouteEntry {
                hostname: host.clone(),
                backend: sr.backend_url.clone(),
                container_id: None,
                container_name: Some("static".into()),
            });
            hb_refs.push(tailsvc_common::api::HeartbeatRouteRef {
                hostname: host.clone(),
                backend_fingerprint: sr.backend.fingerprint(),
            });
            written += 1;
        }
    }
    (written, overridden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StaticRouteConfig;
    use std::collections::HashMap;
    use tailsvc_common::backend::Backend;

    #[test]
    fn prepare_skips_bad_and_keeps_good() {
        let entries = vec![
            StaticRouteConfig {
                hosts: vec!["Admin.Example.com".into(), "bad host".into()],
                backend: "http://127.0.0.1:18080".into(),
            },
            StaticRouteConfig {
                hosts: vec!["x.internal".into()],
                backend: "https://nope".into(),
            },
        ];
        let p = prepare_static_routes(&entries);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].hosts, vec!["admin.example.com".to_string()]);
    }

    #[test]
    fn static_overrides_docker() {
        let mut local = HashMap::new();
        local.insert(
            "admin.example.com".into(),
            BackendRoute {
                backend: Backend {
                    host: "1.2.3.4".into(),
                    port: 9,
                },
                container_id: Some("c".into()),
                container_name: Some("old".into()),
            },
        );
        let mut api = vec![RouteEntry {
            hostname: "admin.example.com".into(),
            backend: "http://1.2.3.4:9/".into(),
            container_id: Some("c".into()),
            container_name: Some("old".into()),
        }];
        let mut hb = vec![];
        let prepared = prepare_static_routes(&[StaticRouteConfig {
            hosts: vec!["admin.example.com".into()],
            backend: "http://127.0.0.1:18080".into(),
        }]);
        let (w, o) = apply_static_routes(&mut local, &mut api, &mut hb, &prepared);
        assert_eq!(w, 1);
        assert_eq!(o, 1);
        assert_eq!(local["admin.example.com"].backend.host, "127.0.0.1");
        assert_eq!(api.len(), 1);
        assert_eq!(api[0].backend, "http://127.0.0.1:18080/");
    }
}
