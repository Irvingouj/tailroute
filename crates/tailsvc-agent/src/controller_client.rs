use anyhow::Context;
use reqwest::Client;
use tailsvc_common::api::{
    EnrollRequest, EnrollResponse, HeartbeatRequest, PutRoutesRequest, PutRoutesResponse,
};

pub struct ControllerClient {
    base: String,
    http: Client,
    agent_id: String,
    token: String,
}

impl ControllerClient {
    pub fn new(base: String, agent_id: String, token: String) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: Client::new(),
            agent_id,
            token,
        }
    }

    pub async fn enroll(
        base: &str,
        enrollment_token: &str,
        req: EnrollRequest,
    ) -> anyhow::Result<EnrollResponse> {
        let url = format!("{}/v1/agents/enroll", base.trim_end_matches('/'));
        let resp = Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {enrollment_token}"))
            .json(&req)
            .send()
            .await
            .context("enroll request")?;
        if !resp.status().is_success() {
            anyhow::bail!("enroll failed: {}", resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn heartbeat(&self, req: HeartbeatRequest) -> anyhow::Result<()> {
        let url = format!("{}/v1/agents/{}/heartbeat", self.base, self.agent_id);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&req)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("heartbeat: {}", resp.status());
        }
        Ok(())
    }

    pub async fn put_routes(&self, req: PutRoutesRequest) -> anyhow::Result<PutRoutesResponse> {
        let url = format!("{}/v1/agents/{}/routes", self.base, self.agent_id);
        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&req)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("put routes: {}", resp.status());
        }
        Ok(resp.json().await?)
    }
}
