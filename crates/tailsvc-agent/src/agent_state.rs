use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAgentState {
    pub agent_id: String,
    pub agent_token: String,
    pub controller_url: String,
}

pub fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join("agent.json")
}

pub fn load(dir: &Path) -> anyhow::Result<Option<PersistedAgentState>> {
    let p = state_path(dir);
    if !p.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&p)?;
    Ok(Some(serde_json::from_str(&data)?))
}

pub fn save(dir: &Path, state: &PersistedAgentState) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let p = state_path(dir);
    std::fs::write(&p, serde_json::to_string_pretty(state)?)?;
    Ok(())
}
