mod backend_resolve;
mod candidate;
mod docker_runtime;
mod types;

pub use backend_resolve::{backend_url, resolve_backend};
pub use candidate::{list_candidates, ContainerCandidate};
pub use docker_runtime::{is_relevant_event, DockerRuntime, EventStream};
pub use types::DiscoveredService;
