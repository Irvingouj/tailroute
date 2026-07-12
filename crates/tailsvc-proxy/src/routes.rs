use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use tailsvc_common::backend::Backend;

#[derive(Clone, Debug)]
pub struct BackendRoute {
    pub backend: Backend,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
}

pub trait RouteStore: Send + Sync {
    fn resolve(&self, host: &str) -> Option<BackendRoute>;
}

#[derive(Default)]
pub struct SharedRouteStore {
    inner: ArcSwap<HashMap<String, BackendRoute>>,
}

impl SharedRouteStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, routes: HashMap<String, BackendRoute>) {
        self.inner.store(Arc::new(routes));
    }
}

impl RouteStore for SharedRouteStore {
    fn resolve(&self, host: &str) -> Option<BackendRoute> {
        let key = host.split(':').next()?.to_ascii_lowercase();
        self.inner.load().get(&key).cloned()
    }
}
