use crate::config::AgentConfig;
use anyhow::{bail, Context};
use std::net::Ipv4Addr;
use std::process::Command;

const TS_PREFIX: &str = "100.";

/// Resolve the IPv4 used for controller registration and (unless `proxy_listen` is set) proxy bind.
pub fn resolve_tailscale_ipv4(cfg: &AgentConfig) -> anyhow::Result<Ipv4Addr> {
    if let Some(ref s) = cfg.tailscale_ipv4 {
        return parse_ts_ip(s.trim());
    }
    discover_tailscale_ipv4()
}

pub fn proxy_listen_addr(
    cfg: &AgentConfig,
    tailscale_ip: Ipv4Addr,
) -> anyhow::Result<std::net::SocketAddr> {
    if let Some(ref s) = cfg.proxy_listen {
        return s
            .parse()
            .with_context(|| format!("invalid proxy_listen: {s}"));
    }
    Ok(format!("{tailscale_ip}:{}", cfg.proxy_port).parse()?)
}

/// SPEC §10 priority: LocalAPI → `tailscale ip -4` → interface inspection.
pub fn discover_tailscale_ipv4() -> anyhow::Result<Ipv4Addr> {
    if let Ok(ip) = from_local_api() {
        return Ok(ip);
    }
    if let Ok(ip) = from_tailscale_cli() {
        return Ok(ip);
    }
    if let Ok(ip) = from_interfaces() {
        return Ok(ip);
    }
    bail!(
        "no tailscale IPv4 found (need 100.x.x.x or set tailscale_ipv4 / TAILSVC_TAILSCALE_IPV4)"
    );
}

/// Tailscale LocalAPI: GET http://local-tailscaled.sock/localapi/v0/status
fn from_local_api() -> anyhow::Result<Ipv4Addr> {
    // Prefer unix socket via curl --unix-socket (portable enough for agent hosts).
    let sock_candidates = [
        "/var/run/tailscale/tailscaled.sock",
        "/var/run/tailscaled.sock",
    ];
    for sock in sock_candidates {
        if !std::path::Path::new(sock).exists() {
            continue;
        }
        let out = Command::new("curl")
            .args([
                "--unix-socket",
                sock,
                "-sS",
                "--max-time",
                "2",
                "http://local-tailscaled.sock/localapi/v0/status",
            ])
            .output()
            .context("curl localapi")?;
        if !out.status.success() {
            continue;
        }
        let body = String::from_utf8_lossy(&out.stdout);
        if let Some(ip) = parse_status_json_ip(&body) {
            return Ok(ip);
        }
    }
    bail!("localapi unavailable")
}

fn parse_status_json_ip(body: &str) -> Option<Ipv4Addr> {
    // Avoid heavy JSON dep in agent for this path: look for Tailscale 100.x Self.TailscaleIPs.
    // Typical: "TailscaleIPs":["100.x.y.z","fd7a:..."]
    let key = "\"TailscaleIPs\"";
    let idx = body.find(key)?;
    let slice = &body[idx..];
    for part in slice.split(['"', '[', ']', ',']) {
        let p = part.trim();
        if p.starts_with("100.") {
            if let Ok(ip) = parse_ts_ip(p) {
                return Some(ip);
            }
        }
    }
    None
}

fn from_tailscale_cli() -> anyhow::Result<Ipv4Addr> {
    let out = Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .context("tailscale ip -4")?;
    if !out.status.success() {
        bail!("tailscale cli failed");
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("").trim();
    parse_ts_ip(line)
}

fn from_interfaces() -> anyhow::Result<Ipv4Addr> {
    let out = Command::new("ip")
        .args(["-4", "addr", "show"])
        .output()
        .context("ip addr")?;
    if !out.status.success() {
        bail!("ip addr failed");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("inet ") {
            let addr = rest.split('/').next().unwrap_or("").trim();
            if let Ok(ip) = parse_ts_ip(addr) {
                return Ok(ip);
            }
        }
    }
    bail!("no tailscale interface")
}

fn parse_ts_ip(s: &str) -> anyhow::Result<Ipv4Addr> {
    let ip: Ipv4Addr = s.parse().context("parse ip")?;
    // Reject obvious non-TS addresses even if they start with 100. (partial CGNAT check).
    if !s.starts_with(TS_PREFIX) {
        bail!("not a tailscale CGNAT address");
    }
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() {
        bail!("invalid address class");
    }
    // Docker bridge often 172.17; public ranges not 100/8.
    Ok(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_localapi_snippet() {
        let body = r#"{"Self":{"TailscaleIPs":["100.64.1.2","fd7a:115c::1"]}}"#;
        let ip = parse_status_json_ip(body).unwrap();
        assert_eq!(ip, Ipv4Addr::new(100, 64, 1, 2));
    }

    #[test]
    fn rejects_non_ts() {
        assert!(parse_ts_ip("127.0.0.1").is_err());
        assert!(parse_ts_ip("192.168.1.1").is_err());
    }
}
