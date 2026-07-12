use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct DnsRegistry {
    inner: Arc<ArcSwap<HashMap<String, Ipv4Addr>>>,
}

impl DnsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, routes: HashMap<String, Ipv4Addr>) {
        self.inner.store(Arc::new(routes));
    }

    pub fn lookup_a(&self, hostname: &str) -> Option<Ipv4Addr> {
        let key = hostname.trim_end_matches('.').to_ascii_lowercase();
        self.inner.load().get(&key).copied()
    }

    pub fn contains(&self, hostname: &str) -> bool {
        self.lookup_a(hostname).is_some()
    }

    /// Test/diagnostic snapshot of registered A targets.
    pub fn snapshot(&self) -> std::collections::HashMap<String, Ipv4Addr> {
        self.inner
            .load()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
}
