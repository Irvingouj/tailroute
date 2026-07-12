# Tailnet Docker Service Router — v1 Technical Specification

## 1. Product Goal

Build a Rust-based system that automatically exposes Docker-hosted services to devices inside the same Tailscale tailnet through arbitrary DNS names.

The system must provide this workflow:

```yaml
services:
  grafana:
    image: grafana/grafana
    labels:
      tailsvc.enable: "true"
      tailsvc.hosts: "grafana.internal,dashboard.internal"
      tailsvc.port: "3000"
```

After the container starts:

```text
http://grafana.internal
http://dashboard.internal
```

must resolve and route directly to the Docker host running that container.

No public domain registration is required.

No public DNS records are required.

No central traffic gateway is allowed.

No per-container Tailscale sidecar is required.

---

# 2. Core Architecture

The system consists of two binaries:

```text
tailsvc-controller
tailsvc-agent
```

## 2.1 Controller

One controller runs inside the tailnet.

Responsibilities:

* Act as the global DNS resolver for the tailnet.
* Maintain the registry of:

  * agents
  * hostnames
  * backend ownership
  * leases
* Return Agent Tailscale IPs for registered hostnames.
* Forward all unregistered DNS queries to configured upstream DNS resolvers.
* Authenticate Agent registrations and updates.
* Detect expired Agents and remove stale DNS records.
* Expose an administrative API.

The Controller is control plane only.

It must never proxy application traffic.

## 2.2 Agent

One Agent runs on every participating Docker host.

Ordinary tailnet devices do not run an Agent.

Responsibilities:

* Read its own current Tailscale IPv4 address.
* Connect to an explicitly configured Controller URL.
* Authenticate and enroll with the Controller.
* Listen to Docker events.
* Discover enabled containers through Docker labels.
* Resolve the correct backend target.
* Register hostnames with the Controller.
* Maintain registration leases through heartbeats.
* Run an HTTP reverse proxy on the Docker host.
* Bind the proxy only to the host's Tailscale IPv4 address.
* Route requests by HTTP `Host` header.
* Remove routes when containers stop or become invalid.

Application traffic always follows:

```text
Tailnet Client
    ↓
Agent on Docker Host
    ↓
Container or explicit backend
```

The Controller must never be in the application data path.

---

# 3. Non-Goals

The following are explicitly out of scope for v1:

* HTTPS termination
* TLS certificate management
* HTTP/2 over TLS
* HTTP/3
* QUIC
* Central gateway mode
* Traffic tunnelling through the Controller
* Public internet exposure
* Kubernetes support
* Podman support
* Container sidecars
* Path-based routing
* Authentication middleware
* Authorization middleware
* Load balancing across multiple Agents
* Multiple backends for one hostname
* Automatic failover between duplicate services
* Public DNS management
* Let's Encrypt
* Internal CA management
* Unix socket backends
* TCP proxying
* UDP proxying
* gRPC-specific configuration
* Service meshes
* Dynamic runtime plugins

The internal architecture must not prevent future HTTPS, HTTP/2, or HTTP/3 support.

---

# 4. Network Model

## 4.1 DNS

The Controller acts as the tailnet's global DNS resolver.

Tailscale DNS must be configured to point tailnet clients to the Controller.

DNS behavior:

```text
DNS query
├── hostname exists in service registry
│   └── return Agent Tailscale IPv4 address
└── hostname does not exist
    └── recursively forward to upstream resolver
```

Default upstream resolvers:

```text
1.1.1.1
8.8.8.8
```

These must be configurable.

The Controller must support:

* UDP DNS on port 53
* TCP DNS on port 53
* Recursive forwarding
* Upstream timeout
* Upstream fallback
* Response caching
* Negative caching
* DNS loop detection
* Case-insensitive hostname matching
* IDNA-normalized hostnames
* A records
* Forwarding of unsupported query types

For registered services, v1 returns only:

```text
A → Agent Tailscale IPv4
```

AAAA records are not generated in v1.

## 4.2 Arbitrary Domain Shadowing

The system must allow registration of any hostname.

Examples:

```text
grafana.internal
service.home
google.com
github.com
example.org
```

Inside the tailnet, the registered hostname resolves to the Agent.

Outside the tailnet, public DNS remains unchanged.

There is no managed-zone restriction.

There is no public domain ownership validation.

A warning may be logged for shadowing public domains, but the registration must not be blocked.

The system must not maintain a mandatory protected-domain list.

## 4.3 Direct Traffic Only

Every registered hostname resolves directly to the Agent hosting its backend.

Example:

```text
grafana.internal
    ↓ DNS
100.82.14.7
    ↓ HTTP
Agent on 100.82.14.7
    ↓
Grafana container
```

No Gateway mode may be implemented.

No Controller proxy fallback may be implemented.

---

# 5. Agent HTTP Proxy

## 5.1 Binding

The Agent must discover its current Tailscale IPv4 address and bind its HTTP listener to:

```text
<TAILSCALE_IPV4>:80
```

It must not bind to:

```text
0.0.0.0:80
127.0.0.1:80
LAN_IP:80
PUBLIC_IP:80
```

If port 80 on the Tailscale address is unavailable, Agent startup must fail with a clear error.

There must be no automatic fallback to another port.

## 5.2 Routing

Routing is based on the normalized HTTP `Host` header.

Each hostname maps to exactly one backend.

Example:

```text
grafana.internal → http://172.18.0.4:3000
api.internal     → http://127.0.0.1:8080
nas.internal     → http://192.168.1.50:5000
```

The proxy must:

* Support HTTP/1.1
* Support WebSocket upgrades
* Support SSE
* Stream request bodies
* Stream response bodies
* Avoid buffering entire bodies
* Preserve the original `Host` header by default
* Add `X-Forwarded-For`
* Add `X-Forwarded-Host`
* Add `X-Forwarded-Proto: http`
* Support configurable connect timeout
* Support configurable response timeout
* Support idle connection reuse
* Disable response caching
* Return deterministic errors
* Shut down gracefully

No path rewrite is supported.

No prefix stripping is supported.

No middleware chain is supported.

## 5.3 Error Responses

Recommended behavior:

```text
Unknown Host header          → 404
Known route, backend down    → 502
Backend timeout              → 504
Malformed request            → 400
Agent shutting down          → 503
```

Responses should include short plain-text bodies.

Example:

```text
unknown tailsvc host
backend unavailable
backend timeout
```

---

# 6. Docker Integration

## 6.1 Runtime Scope

v1 supports Docker Engine only.

The Agent communicates through the Docker Engine API.

Default socket:

```text
/var/run/docker.sock
```

The socket path must be configurable.

Internally, Docker-specific code must be isolated behind a narrow runtime interface.

Suggested interface:

```rust
#[async_trait]
pub trait Runtime {
    async fn list_services(&self) -> Result<Vec<DiscoveredService>>;
    async fn watch_events(&self) -> Result<EventStream>;
    async fn inspect_service(&self, id: &str) -> Result<DiscoveredService>;
}
```

Do not implement dynamic runtime plugin loading in v1.

## 6.2 Discovery Trigger

A container is exposed only when explicitly enabled.

Required label:

```text
tailsvc.enable=true
```

Containers without this label must be ignored.

The Agent must perform:

1. Initial full container scan at startup.
2. Continuous Docker event listening.
3. Reconciliation after reconnecting to Docker.
4. Full rescan after event stream interruption.

Relevant Docker events:

```text
start
stop
die
destroy
rename
connect
disconnect
health_status
```

## 6.3 Labels

Supported labels:

```text
tailsvc.enable
tailsvc.hosts
tailsvc.port
tailsvc.backend
tailsvc.network
```

### `tailsvc.enable`

```yaml
tailsvc.enable: "true"
```

Only the exact case-insensitive boolean value `true` enables exposure.

### `tailsvc.hosts`

Comma-separated hostname list.

```yaml
tailsvc.hosts: "grafana.internal,dashboard.internal"
```

Rules:

* At least one hostname is required.
* Whitespace is trimmed.
* Hostnames are normalized to lowercase.
* Duplicates are removed.
* Ports are not allowed in hostnames.
* URL schemes are not allowed.
* Paths are not allowed.
* Invalid hostname syntax rejects the entire container registration.
* IDNA hostnames must be normalized consistently.

One container may register multiple hostnames.

One hostname may belong to only one Agent at a time.

### `tailsvc.port`

Container-side or host-network backend port.

```yaml
tailsvc.port: "3000"
```

Rules:

* Must be an integer from 1 to 65535.
* Required unless:

  * `tailsvc.backend` is specified, or
  * an unambiguous published port can be inferred, or
  * exactly one exposed TCP port exists.

If multiple candidate ports exist, registration must fail.

### `tailsvc.backend`

Explicit backend URL.

```yaml
tailsvc.backend: "http://127.0.0.1:3000"
```

Supported format in v1:

```text
http://host:port
```

Supported targets include:

* localhost services
* host-network Docker containers
* LAN services
* VMs
* NAS services
* another local process
* arbitrary reachable HTTP backends

Unsupported:

* `https://`
* Unix sockets
* paths
* query parameters
* credentials in URLs
* TCP-only backends

When present, `tailsvc.backend` overrides all automatic backend discovery.

### `tailsvc.network`

Select a Docker network when multiple container networks are present.

```yaml
tailsvc.network: "frontend"
```

If the requested network does not exist, registration fails.

---

# 7. Backend Resolution

Backend resolution priority must be:

```text
1. Explicit tailsvc.backend
2. Published host port
3. network_mode=host
4. Container network IP
5. Reject as ambiguous or unreachable
```

## 7.1 Explicit Backend

Example:

```yaml
labels:
  tailsvc.backend: "http://192.168.1.50:5000"
```

Resolved target:

```text
192.168.1.50:5000
```

No Docker network inspection is required after validation.

## 7.2 Published Port

Example:

```yaml
ports:
  - "127.0.0.1:8080:3000"
```

Resolved target:

```text
127.0.0.1:8080
```

Published host bindings take priority over direct container IP access.

If the published host address is:

```text
0.0.0.0
```

the Agent should resolve it to:

```text
127.0.0.1
```

for local proxy access.

If multiple published host bindings exist for the chosen container port, registration must fail unless exactly one safe local binding can be selected deterministically.

## 7.3 Host Network

For:

```yaml
network_mode: host
```

the default backend is:

```text
127.0.0.1:<tailsvc.port>
```

No attempt should be made to connect to `0.0.0.0`.

The user may override this with `tailsvc.backend`.

## 7.4 Container Network IP

For bridge or user-defined Docker networks:

```text
<container-ip>:<tailsvc.port>
```

Rules:

* If `tailsvc.network` is specified, use that network.
* If only one usable network exists, use it.
* If multiple usable networks exist and none is explicitly selected, reject registration.
* Do not select the first network arbitrarily.
* Do not modify Docker network membership.
* Do not automatically attach the Agent container to application networks.
* Do not mutate the target container.

## 7.5 Connectivity Validation

Before registering a route, the Agent should perform a lightweight TCP connection test.

Recommended timeout:

```text
2 seconds
```

A failed initial probe should prevent registration unless disabled by configuration.

Continuous backend health checking is optional for v1.

At minimum, proxy-time failures must be surfaced as `502` or `504`.

---

# 8. Hostname Ownership

Hostname ownership is globally unique.

## 8.1 Registration Rule

The first healthy Agent to register a hostname owns it.

A second Agent attempting to register the same hostname receives:

```text
409 Conflict
```

The conflict response must include:

* normalized hostname
* current owner Agent ID
* lease expiry time

It must not expose secrets.

## 8.2 Renewal

The owning Agent may:

* renew the hostname lease
* change the backend
* add hostnames
* remove hostnames
* update container metadata

## 8.3 Release

A hostname is released when:

* the container stops
* the container is destroyed
* the label is removed
* the Agent explicitly unregisters it
* the Agent lease expires
* an administrator forcibly revokes it

## 8.4 No Load Balancing

A hostname maps to exactly one Agent.

The Controller must not return multiple A records for the same registered hostname.

---

# 9. Agent Enrollment and Authentication

## 9.1 Enrollment

The Agent is configured with:

```text
controller URL
one-time enrollment token
```

Example:

```text
TAILSVC_CONTROLLER=http://100.80.10.5:8080
TAILSVC_ENROLLMENT_TOKEN=...
```

Enrollment flow:

```text
Agent
  ↓ POST /v1/agents/enroll
Controller validates one-time token
  ↓
Controller creates Agent identity
  ↓
Controller returns long-lived Agent credential
  ↓
Agent stores credential locally
```

The enrollment token must become invalid after successful use.

## 9.2 Long-Lived Agent Credential

The Agent uses its credential for:

* heartbeat
* route registration
* route updates
* route deletion
* Agent metadata updates

The credential must be:

* random
* high entropy
* revocable
* stored hashed by Controller
* stored securely by Agent
* excluded from logs

Bearer-token authentication is acceptable for v1.

## 9.3 Agent Identity

Each Agent has:

```text
agent_id
display_name
tailscale_ipv4
hostname
docker_engine_id
created_at
last_seen_at
status
```

The Controller must treat the latest authenticated heartbeat as the source of truth for current Tailscale IPv4.

The Agent must read its current Tailscale IP itself.

The IP must not require manual configuration.

---

# 10. Tailscale IP Discovery

The Agent must discover its own Tailscale IPv4 through one or more supported methods.

Recommended priority:

```text
1. Tailscale LocalAPI
2. `tailscale ip -4`
3. Interface inspection fallback
```

LocalAPI is preferred.

The discovery implementation must reject:

* loopback addresses
* Docker bridge addresses
* LAN addresses
* public addresses
* non-Tailscale interfaces

If no Tailscale IPv4 is available, Agent startup must fail.

If the Tailscale IPv4 changes while running:

1. Stop accepting new traffic on the old address.
2. Bind to the new address.
3. Update the Controller.
4. Renew all route registrations.
5. Close the old listener after a short drain period.

---

# 11. Controller API

Suggested HTTP API.

## 11.1 Agent Enrollment

```http
POST /v1/agents/enroll
Authorization: Bearer <enrollment-token>
Content-Type: application/json
```

Request:

```json
{
  "display_name": "docker-host-a",
  "tailscale_ipv4": "100.82.14.7",
  "docker_engine_id": "..."
}
```

Response:

```json
{
  "agent_id": "agt_...",
  "agent_token": "...",
  "heartbeat_interval_seconds": 20,
  "lease_ttl_seconds": 90
}
```

## 11.2 Heartbeat

```http
POST /v1/agents/{agent_id}/heartbeat
Authorization: Bearer <agent-token>
```

Request:

```json
{
  "tailscale_ipv4": "100.82.14.7",
  "routes": [
    {
      "hostname": "grafana.internal",
      "backend_fingerprint": "..."
    }
  ]
}
```

## 11.3 Register Routes

```http
PUT /v1/agents/{agent_id}/routes
Authorization: Bearer <agent-token>
```

Request:

```json
{
  "routes": [
    {
      "hostname": "grafana.internal",
      "backend": "http://172.18.0.4:3000",
      "container_id": "...",
      "container_name": "grafana"
    }
  ]
}
```

The Controller should treat this request as the Agent's desired complete route set.

This is preferred over many incremental mutation calls because reconciliation becomes deterministic.

Response:

```json
{
  "accepted": [
    "grafana.internal"
  ],
  "conflicts": []
}
```

Conflict example:

```json
{
  "accepted": [],
  "conflicts": [
    {
      "hostname": "grafana.internal",
      "owner_agent_id": "agt_other",
      "lease_expires_at": "..."
    }
  ]
}
```

## 11.4 Administrative Endpoints

Minimum administrative API:

```text
GET    /v1/admin/agents
GET    /v1/admin/routes
DELETE /v1/admin/routes/{hostname}
POST   /v1/admin/enrollment-tokens
DELETE /v1/admin/agents/{agent_id}
GET    /health
GET    /ready
```

Admin authentication may use a separate static admin token in v1.

---

# 12. Lease Model

Recommended defaults:

```text
heartbeat interval: 20 seconds
Agent lease TTL: 90 seconds
service DNS TTL: 5 seconds
grace period: none beyond lease TTL
```

Rules:

* Every successful heartbeat renews the Agent lease.
* Every route owned by an Agent inherits that Agent lease.
* Expired Agents are excluded from DNS immediately.
* Expired routes become reclaimable.
* A background Controller task removes expired state.
* Agent reconnection must reconcile the full route set.
* The Agent must tolerate temporary Controller failures.
* Existing proxy routes should continue serving while the Controller is unreachable.
* The Agent must keep retrying heartbeats with bounded exponential backoff.
* The Controller outage must not stop local proxy traffic.
* New DNS resolution may fail while Controller DNS is unavailable.

---

# 13. DNS Behavior

## 13.1 Registered Hostname

Query:

```text
grafana.internal A
```

Response:

```text
grafana.internal. 5 IN A 100.82.14.7
```

## 13.2 Unregistered Hostname

Query is forwarded to upstream DNS.

The Controller must preserve:

* query name
* query type
* recursion desired flag
* response code
* relevant DNSSEC flags where supported

## 13.3 Registered Hostname Non-A Query

For a registered hostname:

* `A`: return Agent Tailscale IPv4
* `AAAA`: return empty successful response in v1
* other record types: forward upstream or return empty response

Recommended rule:

```text
A     → internal registry
AAAA  → NODATA
other → upstream
```

This avoids accidentally returning public A records while still permitting unrelated TXT/MX lookups.

## 13.4 Caching

Recommended cache policy:

```text
registered A records: generated dynamically, TTL 5 seconds
positive upstream responses: respect TTL, cap at 300 seconds
negative upstream responses: cap at 30 seconds
upstream timeout: 2 seconds
overall query timeout: 5 seconds
```

## 13.5 DNS Failure

If all upstream resolvers fail:

```text
SERVFAIL
```

Do not fabricate NXDOMAIN.

---

# 14. Persistence

## 14.1 Controller Storage

Use SQLite for v1.

Persist:

* Agents
* Agent credential hashes
* Enrollment tokens
* Route ownership
* Route metadata
* Audit timestamps
* Administrative revocations

Do not rely on SQLite rows alone for liveness.

Lease validity is calculated from timestamps.

Suggested tables:

```text
agents
agent_credentials
enrollment_tokens
routes
audit_events
```

## 14.2 Agent Storage

Persist:

* Agent ID
* Agent token
* Controller URL
* Last known configuration
* Optional local route snapshot

Recommended path:

```text
/var/lib/tailsvc-agent
```

The Agent must recover after restart without requiring re-enrollment.

---

# 15. Configuration

## 15.1 Controller Configuration

Example:

```toml
[dns]
listen = "100.80.10.5:53"
upstreams = ["1.1.1.1:53", "8.8.8.8:53"]
service_ttl_seconds = 5
positive_cache_max_seconds = 300
negative_cache_max_seconds = 30
upstream_timeout_ms = 2000

[api]
listen = "100.80.10.5:8080"
admin_token_file = "/run/secrets/admin_token"

[storage]
sqlite_path = "/var/lib/tailsvc-controller/controller.db"

[leases]
heartbeat_interval_seconds = 20
agent_ttl_seconds = 90
```

The Controller should bind only to its Tailscale IP unless explicitly overridden.

## 15.2 Agent Configuration

Example:

```toml
controller_url = "http://100.80.10.5:8080"
docker_socket = "/var/run/docker.sock"
state_dir = "/var/lib/tailsvc-agent"
proxy_port = 80
connect_timeout_ms = 2000
response_timeout_seconds = 300
```

Environment-variable overrides should be supported.

Suggested variables:

```text
TAILSVC_CONTROLLER
TAILSVC_ENROLLMENT_TOKEN
TAILSVC_DOCKER_SOCKET
TAILSVC_STATE_DIR
TAILSVC_PROXY_PORT
RUST_LOG
```

---

# 16. Deployment

## 16.1 Controller Container

Example:

```yaml
services:
  controller:
    image: ghcr.io/example/tailsvc-controller:latest
    network_mode: host
    restart: unless-stopped
    volumes:
      - controller-data:/var/lib/tailsvc-controller
    environment:
      TAILSVC_CONFIG: /etc/tailsvc/controller.toml
    cap_add:
      - NET_BIND_SERVICE

volumes:
  controller-data:
```

The host must already be connected to Tailscale.

The user configures Tailscale DNS once so the Controller becomes the global nameserver.

## 16.2 Agent Container

Example:

```yaml
services:
  tailsvc-agent:
    image: ghcr.io/example/tailsvc-agent:latest
    network_mode: host
    restart: unless-stopped
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - tailsvc-agent-data:/var/lib/tailsvc-agent
    environment:
      TAILSVC_CONTROLLER: "http://100.80.10.5:8080"
      TAILSVC_ENROLLMENT_TOKEN: "..."
    cap_add:
      - NET_BIND_SERVICE

volumes:
  tailsvc-agent-data:
```

The Agent container uses host networking so it can:

* bind directly to the host Tailscale address
* access localhost published ports
* access host-network services
* access Docker bridge addresses where host routing permits

---

# 17. Rust Implementation Guidance

## 17.1 Workspace Layout

Recommended Cargo workspace:

```text
tailsvc/
├── Cargo.toml
├── crates/
│   ├── tailsvc-common/
│   ├── tailsvc-controller/
│   ├── tailsvc-agent/
│   ├── tailsvc-dns/
│   ├── tailsvc-proxy/
│   ├── tailsvc-docker/
│   └── tailsvc-storage/
├── config/
├── deploy/
└── tests/
```

## 17.2 Suggested Responsibilities

### `tailsvc-common`

* Shared models
* Hostname normalization
* API DTOs
* Authentication helpers
* Error types
* Configuration primitives

### `tailsvc-controller`

* API server
* Agent enrollment
* Heartbeats
* Route ownership
* Lease cleanup
* Admin endpoints
* Controller orchestration

### `tailsvc-agent`

* Tailscale IP discovery
* Controller client
* Reconciliation loop
* State persistence
* Process lifecycle

### `tailsvc-dns`

* UDP/TCP DNS server
* Registry lookup
* Recursive forwarding
* Cache
* Upstream fallback

### `tailsvc-proxy`

* Dynamic host routing
* Reverse proxy
* WebSocket support
* Streaming
* Timeouts
* Graceful shutdown

### `tailsvc-docker`

* Docker API client
* Event stream
* Label parsing
* Backend resolution
* Runtime abstraction

### `tailsvc-storage`

* SQLite schema
* Migrations
* Queries
* Credential hashing
* Transactions

## 17.3 Framework Direction

Use existing protocol implementations.

Do not implement HTTP parsing, DNS packet parsing, WebSocket framing, or Docker wire protocols manually.

Appropriate categories:

* Async runtime: Tokio
* HTTP server/client: Hyper-based stack
* API routing: Axum
* Docker Engine API: Bollard or equivalent mature crate
* DNS server/client: Hickory DNS
* SQLite: SQLx
* Serialization: Serde
* CLI/config: Clap and Figment/config
* Logging/tracing: tracing
* Credential hashing: Argon2 or SHA-256 for random bearer tokens
* Secret generation: OS CSPRNG
* IDNA: idna crate
* Retry: bounded exponential backoff

Exact crate versions should be selected from current stable releases during implementation.

## 17.4 Proxy Abstraction

The proxy implementation should expose a dynamic routing API:

```rust
pub trait RouteStore {
    fn resolve(&self, host: &str) -> Option<Backend>;
}
```

Route updates must be atomic.

Recommended storage:

```rust
ArcSwap<HashMap<NormalizedHostname, Backend>>
```

or another lock-efficient immutable snapshot approach.

The proxy must not reload the process for route updates.

## 17.5 Reconciliation Model

The Agent should use desired-state reconciliation.

Pseudo-flow:

```text
startup
→ load Agent credentials
→ discover Tailscale IP
→ connect Docker
→ list enabled containers
→ resolve desired routes
→ update local proxy route table
→ send complete desired route set to Controller
→ watch Docker events
→ reconcile on every relevant event
→ heartbeat periodically
```

Do not rely exclusively on Docker event deltas.

Every event should trigger debounced reconciliation.

Recommended debounce:

```text
250–500 ms
```

This avoids race conditions during Compose startup.

---

# 18. Security Requirements

## 18.1 Binding

Controller DNS, Controller API, and Agent proxy should bind only to Tailscale IPs by default.

No service may silently fall back to `0.0.0.0`.

## 18.2 Docker Socket

Access to Docker socket is equivalent to root-level control of the Docker host.

The documentation must state this clearly.

The Agent must:

* never expose Docker API remotely
* never proxy Docker socket access
* never allow labels to invoke commands
* never mutate containers
* never mount arbitrary paths
* treat all label values as untrusted input

## 18.3 SSRF

`tailsvc.backend` permits arbitrary HTTP targets.

This is intentional.

The security model assumes the administrator controlling Docker labels is trusted.

Still, the Agent must reject:

* malformed URLs
* URLs with embedded credentials
* unsupported schemes
* invalid ports
* control characters
* header injection attempts

## 18.4 Credentials

* Never log Agent tokens.
* Never return credential hashes.
* Hash stored long-lived credentials.
* Enrollment tokens must be one-time use.
* Admin token must be separate from Agent tokens.
* Constant-time comparisons should be used where applicable.

## 18.5 DNS Shadowing

Shadowing arbitrary public domains is intentional.

The system may emit warnings, but must allow it.

Documentation must warn that shadowing authentication, update, package registry, or Tailscale control-plane domains can break client behavior.

---

# 19. Observability

Both binaries must expose structured logs.

Recommended fields:

```text
agent_id
hostname
container_id
container_name
backend
tailscale_ip
event
duration_ms
status
error
```

Metrics are optional for v1 but architecture should permit later Prometheus support.

Minimum health endpoints:

```text
/health
/ready
```

Controller readiness requires:

* SQLite available
* DNS listener active
* API listener active

Agent readiness requires:

* Tailscale IP discovered
* Docker connected
* proxy listener active
* enrolled with Controller
* initial reconciliation completed

---

# 20. Testing Requirements

## 20.1 Unit Tests

Must cover:

* Hostname normalization
* IDNA handling
* Label parsing
* Backend URL validation
* Backend resolution priority
* Port inference
* Multi-network ambiguity
* Hostname ownership conflicts
* Lease expiration
* DNS registry lookup
* Upstream DNS fallback
* Credential validation
* Route reconciliation

## 20.2 Integration Tests

Use Docker-based tests.

Required cases:

1. Bridge-network container exposed through Agent.
2. Published localhost port.
3. Host-network container.
4. Explicit LAN backend.
5. Multiple hostnames for one container.
6. Duplicate hostname conflict.
7. Container stop removes DNS record.
8. Agent crash expires lease.
9. Unknown domain forwards to upstream DNS.
10. WebSocket proxying.
11. SSE streaming.
12. Large streaming request and response.
13. Docker event stream reconnect.
14. Agent restart without re-enrollment.
15. Tailscale IP change simulation.
16. Controller restart with persisted state.
17. Public-domain shadowing.
18. Invalid labels do not affect valid routes.

## 20.3 End-to-End Acceptance Test

Given:

```yaml
services:
  whoami:
    image: traefik/whoami
    labels:
      tailsvc.enable: "true"
      tailsvc.hosts: "whoami.internal"
      tailsvc.port: "80"
```

When:

```bash
docker compose up -d
```

Then, from another tailnet device:

```bash
curl http://whoami.internal
```

must reach the `whoami` container directly through the Agent on that Docker host.

Stopping the container must cause the hostname to stop resolving within:

```text
Agent event propagation + DNS TTL
```

Target:

```text
under 10 seconds
```

---

# 21. v1 Acceptance Criteria

v1 is complete only when all of the following are true:

* One Controller can serve DNS for the entire tailnet.
* Unknown DNS queries are forwarded correctly.
* Any arbitrary hostname can be registered.
* Each participating Docker host runs one Agent.
* Agent automatically discovers its Tailscale IPv4.
* Agent binds HTTP only to that Tailscale IP.
* Docker containers are discovered through labels.
* One container may expose multiple hostnames.
* Explicit backends work.
* Published host ports work.
* Host-network containers work.
* Bridge-network container IPs work.
* Multiple Docker networks require explicit selection when ambiguous.
* Hostname ownership conflicts are deterministic.
* Agent leases remove stale DNS records.
* Existing traffic does not depend on Controller proxying.
* WebSocket, SSE, and streaming HTTP work.
* Agent restart does not require re-enrollment.
* Controller restart preserves registered identity and ownership state.
* No public domain or certificate setup is required.
* No component binds publicly by default.
* Automated tests cover the primary routing paths.

---

# 22. Future Work

Potential v2 features:

* HTTPS with an internal CA
* Optional public CA support for owned domains
* HTTP/2
* HTTP/3 over QUIC
* Tailscale identity verification through LocalAPI
* Multiple Controllers
* DNS high availability
* IPv6
* Health checks
* Route priorities
* Weighted backends
* Agent draining
* Admin web UI
* Prometheus metrics
* Podman runtime
* Non-Docker static route declarations
* TCP services
* UDP services
* Access-control integration
* Audit log export
* Per-route authentication
* Automatic Tailscale DNS configuration

These must remain separate from the v1 implementation.