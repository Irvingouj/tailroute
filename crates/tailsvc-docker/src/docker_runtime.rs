use crate::types::{DiscoveredService, PublishedPort};
use bollard::container::ListContainersOptions;
use bollard::models::EventMessage;
use bollard::system::EventsOptions;
use bollard::Docker;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use tailsvc_common::labels::{labels_from_map, validate_service_labels};
use tracing::warn;

pub type EventStream =
    Pin<Box<dyn Stream<Item = Result<EventMessage, bollard::errors::Error>> + Send>>;

pub struct DockerRuntime {
    docker: Docker,
}

impl DockerRuntime {
    pub fn connect(socket: &str) -> Result<Self, bollard::errors::Error> {
        let docker = if socket == "/var/run/docker.sock" || socket.is_empty() {
            Docker::connect_with_socket_defaults()?
        } else {
            Docker::connect_with_unix(socket, 120, bollard::API_DEFAULT_VERSION)?
        };
        Ok(Self { docker })
    }

    /// List only **running** containers with valid tailsvc labels.
    pub async fn list_services(&self) -> Result<Vec<DiscoveredService>, bollard::errors::Error> {
        let mut filters = HashMap::new();
        filters.insert("status".to_string(), vec!["running".to_string()]);
        let containers = self
            .docker
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
            match self.inspect_service(&id).await {
                Ok(Some(s)) => out.push(s),
                Ok(None) => {}
                Err(e) => warn!(container_id = %id, error = %e, "inspect failed"),
            }
        }
        Ok(out)
    }

    pub async fn inspect_service(
        &self,
        id: &str,
    ) -> Result<Option<DiscoveredService>, bollard::errors::Error> {
        let inspect = self.docker.inspect_container(id, None).await?;

        // Only expose running containers.
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
        let labels = match labels_from_map(&labels_raw) {
            Some(l) => l,
            None => return Ok(None),
        };
        if validate_service_labels(&labels).is_err() {
            return Ok(None);
        }

        let name = inspect
            .name
            .clone()
            .unwrap_or_else(|| id.to_string())
            .trim_start_matches('/')
            .to_string();

        let network_mode = inspect
            .host_config
            .as_ref()
            .and_then(|h| h.network_mode.clone());

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
                        if let (Some(hp), Some(hi)) = (&b.host_port, &b.host_ip) {
                            if let Ok(hport) = hp.parse::<u16>() {
                                published_ports.push(PublishedPort {
                                    container_port,
                                    host_ip: hi.clone(),
                                    host_port: hport,
                                });
                            }
                        } else if let Some(hp) = &b.host_port {
                            // host_ip may be empty → treat as 0.0.0.0
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
            for (name, net) in nets {
                if let Some(ip) = net.ip_address.filter(|s| !s.is_empty()) {
                    networks.insert(name, ip);
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

        Ok(Some(DiscoveredService {
            container_id: id.to_string(),
            container_name: name,
            labels,
            network_mode,
            published_ports,
            networks,
            exposed_ports,
        }))
    }

    /// Watch Docker events; caller should reconnect on stream end.
    pub fn watch_events(&self) -> EventStream {
        let mut filters = HashMap::new();
        filters.insert(
            "type".to_string(),
            vec!["container".to_string(), "network".to_string()],
        );
        let opts = EventsOptions {
            filters,
            ..Default::default()
        };
        Box::pin(self.docker.events(Some(opts)))
    }

    pub async fn ping(&self) -> Result<(), bollard::errors::Error> {
        self.docker.ping().await.map(|_| ())
    }

    pub fn docker(&self) -> &Docker {
        &self.docker
    }
}

/// Relevant Docker event actions per SPEC §6.2.
pub fn is_relevant_event(msg: &EventMessage) -> bool {
    let action = msg.action.as_deref().unwrap_or("");
    matches!(
        action,
        "start"
            | "stop"
            | "die"
            | "destroy"
            | "rename"
            | "connect"
            | "disconnect"
            | "health_status"
            | "health_status: healthy"
            | "health_status: unhealthy"
    ) || action.starts_with("health_status")
}
