//! Full-container discovery for admin "click to enable" (not only tailsvc labels).

use crate::types::{DiscoveredService, PublishedPort};
use bollard::container::ListContainersOptions;
use bollard::Docker;
use std::collections::HashMap;
use tailsvc_common::api::DiscoveredContainerDto;
use tailsvc_common::backend::Backend;
use tailsvc_common::labels::labels_from_map;
use tracing::warn;

const SKIP_IMAGE_PREFIXES: &[&str] = &[
    "redis:",
    "postgres:",
    "mysql:",
    "mongo:",
    "mariadb:",
    "memcached:",
    "rabbitmq:",
    "pgvector/",
    "tensorchord/",
];

#[derive(Clone, Debug)]
pub struct ContainerCandidate {
    pub identity_key: String,
    pub container_id: String,
    pub container_name: String,
    pub image: String,
    pub network_mode: Option<String>,
    pub published_ports: Vec<PublishedPort>,
    pub exposed_ports: Vec<u16>,
    pub networks: HashMap<String, String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub has_tailsvc_labels: bool,
    pub suggested_backend: Option<Backend>,
}

impl ContainerCandidate {
    pub fn to_dto(&self) -> DiscoveredContainerDto {
        DiscoveredContainerDto {
            identity_key: self.identity_key.clone(),
            container_id: self.container_id.clone(),
            container_name: self.container_name.clone(),
            image: self.image.clone(),
            network_mode: self.network_mode.clone(),
            suggested_backend: self
                .suggested_backend
                .as_ref()
                .map(|b| b.base_url().trim_end_matches('/').to_string()),
            published_ports: {
                let mut p: Vec<u16> = self.published_ports.iter().map(|x| x.host_port).collect();
                p.sort_unstable();
                p.dedup();
                p
            },
            exposed_ports: self.exposed_ports.clone(),
            compose_project: self.compose_project.clone(),
            compose_service: self.compose_service.clone(),
            has_tailsvc_labels: self.has_tailsvc_labels,
        }
    }
}

pub async fn list_candidates(
    docker: &Docker,
) -> Result<Vec<ContainerCandidate>, bollard::errors::Error> {
    let mut filters = HashMap::new();
    filters.insert("status".to_string(), vec!["running".to_string()]);
    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: false,
            filters,
            ..Default::default()
        }))
        .await?;

    let mut out = Vec::new();
    for summary in containers {
        let id = match summary.id {
            Some(id) => id,
            None => continue,
        };
        match inspect_candidate(docker, &id).await {
            Ok(Some(c)) => {
                if should_skip_image(&c.image) {
                    continue;
                }
                out.push(c);
            }
            Ok(None) => {}
            Err(e) => warn!(container_id = %id, error = %e, "candidate inspect failed"),
        }
    }
    out.sort_by(|a, b| a.container_name.cmp(&b.container_name));
    Ok(out)
}

fn should_skip_image(image: &str) -> bool {
    let lower = image.to_ascii_lowercase();
    SKIP_IMAGE_PREFIXES.iter().any(|p| lower.contains(p))
}

async fn inspect_candidate(
    docker: &Docker,
    id: &str,
) -> Result<Option<ContainerCandidate>, bollard::errors::Error> {
    let inspect = docker.inspect_container(id, None).await?;
    let running = inspect
        .state
        .as_ref()
        .and_then(|s| s.running)
        .unwrap_or(false);
    if !running {
        return Ok(None);
    }

    let labels_raw = inspect
        .config
        .as_ref()
        .and_then(|c| c.labels.as_ref())
        .cloned()
        .unwrap_or_default();

    let name = inspect
        .name
        .clone()
        .unwrap_or_else(|| id.to_string())
        .trim_start_matches('/')
        .to_string();

    let image = inspect
        .config
        .as_ref()
        .and_then(|c| c.image.clone())
        .unwrap_or_default();

    let network_mode = inspect
        .host_config
        .as_ref()
        .and_then(|h| h.network_mode.clone());

    let compose_project = labels_raw.get("com.docker.compose.project").cloned();
    let compose_service = labels_raw.get("com.docker.compose.service").cloned();

    let identity_key = match (&compose_project, &compose_service) {
        (Some(p), Some(s)) => format!("compose:{p}/{s}"),
        _ => format!("name:{name}"),
    };

    let has_tailsvc_labels = labels_from_map(&labels_raw).is_some();

    let network_settings = inspect.network_settings.clone();
    let mut published_ports = Vec::new();
    if let Some(ports) = network_settings.as_ref().and_then(|n| n.ports.clone()) {
        for (key, bindings) in ports {
            let container_port = key
                .split('/')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(0);
            if let Some(binds) = bindings {
                for b in binds {
                    if let Some(hp) = &b.host_port {
                        if let Ok(hport) = hp.parse::<u16>() {
                            published_ports.push(PublishedPort {
                                container_port,
                                host_ip: b.host_ip.clone().unwrap_or_else(|| "0.0.0.0".into()),
                                host_port: hport,
                            });
                        }
                    }
                }
            }
        }
    }

    let mut networks = HashMap::new();
    if let Some(nets) = network_settings.as_ref().and_then(|n| n.networks.clone()) {
        for (nname, net) in nets {
            if let Some(ip) = net.ip_address.filter(|s| !s.is_empty()) {
                networks.insert(nname, ip);
            }
        }
    }

    let mut exposed_ports = Vec::new();
    if let Some(config) = &inspect.config {
        if let Some(ep) = &config.exposed_ports {
            for key in ep.keys() {
                if let Some(p) = key.split('/').next().and_then(|x| x.parse().ok()) {
                    exposed_ports.push(p);
                }
            }
        }
    }
    exposed_ports.sort_unstable();
    exposed_ports.dedup();

    let suggested_backend =
        suggest_backend(&network_mode, &published_ports, &exposed_ports, &networks);

    Ok(Some(ContainerCandidate {
        identity_key,
        container_id: id.to_string(),
        container_name: name,
        image,
        network_mode,
        published_ports,
        exposed_ports,
        networks,
        compose_project,
        compose_service,
        has_tailsvc_labels,
        suggested_backend,
    }))
}

fn suggest_backend(
    network_mode: &Option<String>,
    published: &[PublishedPort],
    exposed: &[u16],
    networks: &HashMap<String, String>,
) -> Option<Backend> {
    // Prefer single published host port
    if published.len() == 1 {
        let p = &published[0];
        let host = if p.host_ip == "0.0.0.0" || p.host_ip.is_empty() {
            "127.0.0.1".into()
        } else {
            p.host_ip.clone()
        };
        return Some(Backend {
            host,
            port: p.host_port,
        });
    }
    if network_mode.as_deref() == Some("host") {
        if exposed.len() == 1 {
            return Some(Backend {
                host: "127.0.0.1".into(),
                port: exposed[0],
            });
        }
        return None;
    }
    if networks.len() == 1 && exposed.len() == 1 {
        let ip = networks.values().next()?.clone();
        return Some(Backend {
            host: ip,
            port: exposed[0],
        });
    }
    if networks.len() == 1 && published.is_empty() && exposed.is_empty() {
        return None;
    }
    let _ = DiscoveredService {
        container_id: String::new(),
        container_name: String::new(),
        labels: tailsvc_common::labels::ServiceLabels {
            hosts: vec![],
            port: None,
            backend: None,
            network: None,
        },
        network_mode: None,
        published_ports: vec![],
        networks: HashMap::new(),
        exposed_ports: vec![],
    };
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_redis() {
        assert!(should_skip_image("redis:7-alpine"));
        assert!(!should_skip_image("traefik/whoami:v1"));
    }

    #[test]
    fn identity_compose() {
        assert_eq!(format!("compose:{}/{}", "lib", "api"), "compose:lib/api");
    }
}
