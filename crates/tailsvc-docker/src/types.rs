use std::collections::HashMap;
use tailsvc_common::labels::ServiceLabels;

#[derive(Clone, Debug)]
pub struct DiscoveredService {
    pub container_id: String,
    pub container_name: String,
    pub labels: ServiceLabels,
    pub network_mode: Option<String>,
    pub published_ports: Vec<PublishedPort>,
    pub networks: HashMap<String, String>, // network name -> container ipv4
    pub exposed_ports: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct PublishedPort {
    pub container_port: u16,
    pub host_ip: String,
    pub host_port: u16,
}
