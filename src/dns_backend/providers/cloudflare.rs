use std::{env, net::IpAddr};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    config::{CloudflareRecordConfig, RecordConfig},
    dns_backend::DnsBackend,
    logger,
};

const API_BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct CloudflareBackend {
    client: reqwest::Client,
    zone_id: String,
    ttl: u32,
    proxied: Option<bool>,
}

impl CloudflareBackend {
    pub fn new(config: &CloudflareRecordConfig) -> Result<Self> {
        let token = match (&config.api_token, &config.api_token_env) {
            (Some(token), _) => token.clone(),
            (None, Some(env_name)) => env::var(env_name)
                .with_context(|| format!("environment variable {env_name} is not set"))?,
            (None, None) => {
                return Err(anyhow!(
                    "cloudflare config requires api_token or api_token_env"
                ));
            }
        };

        let mut headers = reqwest::header::HeaderMap::new();
        let auth_value = format!("Bearer {token}")
            .parse()
            .context("failed to build Cloudflare authorization header")?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build Cloudflare HTTP client")?;

        Ok(Self {
            client,
            zone_id: config.zone_id.clone(),
            ttl: config.ttl,
            proxied: config.proxied,
        })
    }

    async fn list_records(&self, record: &RecordConfig) -> Result<Vec<CloudflareDnsRecord>> {
        let url = format!("{API_BASE}/zones/{}/dns_records", self.zone_id);
        let response = self
            .client
            .get(&url)
            .query(&[
                ("type", record.record_type.as_dns_type()),
                ("name", record.name.as_str()),
            ])
            .send()
            .await
            .with_context(|| format!("failed to list Cloudflare DNS records for {}", record.name))?
            .error_for_status()
            .with_context(|| {
                format!(
                    "Cloudflare returned an error status while listing {}",
                    record.name
                )
            })?
            .json::<CloudflareResponse<Vec<CloudflareDnsRecord>>>()
            .await
            .context("failed to decode Cloudflare list response")?;

        response.into_result()
    }

    async fn create_record(&self, record: &RecordConfig, ip: IpAddr) -> Result<()> {
        let url = format!("{API_BASE}/zones/{}/dns_records", self.zone_id);
        self.send_record_request(self.client.post(url), record, ip)
            .await
            .with_context(|| {
                format!(
                    "failed to create {} {}",
                    record.record_type.as_dns_type(),
                    record.name
                )
            })
    }

    async fn update_record(
        &self,
        record: &RecordConfig,
        cloudflare_record_id: &str,
        ip: IpAddr,
    ) -> Result<()> {
        let url = format!(
            "{API_BASE}/zones/{}/dns_records/{}",
            self.zone_id, cloudflare_record_id
        );
        self.send_record_request(self.client.put(url), record, ip)
            .await
            .with_context(|| {
                format!(
                    "failed to update {} {}",
                    record.record_type.as_dns_type(),
                    record.name
                )
            })
    }

    async fn send_record_request(
        &self,
        request: reqwest::RequestBuilder,
        record: &RecordConfig,
        ip: IpAddr,
    ) -> Result<()> {
        let payload = CloudflareDnsRecordPayload {
            record_type: record.record_type.as_dns_type(),
            name: &record.name,
            content: ip.to_string(),
            ttl: self.ttl,
            proxied: self.proxied,
        };

        let response = request
            .json(&payload)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudflareResponse<CloudflareDnsRecord>>()
            .await
            .context("failed to decode Cloudflare write response")?;

        response.into_result().map(|_| ())
    }
}

#[async_trait]
impl DnsBackend for CloudflareBackend {
    fn name(&self) -> &'static str {
        "cloudflare"
    }

    async fn upsert_record(&self, record: &RecordConfig, ip: IpAddr) -> Result<()> {
        let records = self.list_records(record).await?;
        if let Some(existing) = records.first() {
            if existing.content == ip.to_string() {
                logger::info(
                    "cloudflare",
                    format!(
                        "dns_record_already_current record_type={} name={} ip={}",
                        record.record_type.as_dns_type(),
                        record.name,
                        ip
                    ),
                );
                return Ok(());
            }

            self.update_record(record, &existing.id, ip).await
        } else {
            self.create_record(record, ip).await
        }
    }
}

#[derive(Debug, Deserialize)]
struct CloudflareResponse<T> {
    success: bool,
    errors: Vec<CloudflareError>,
    result: Option<T>,
}

impl<T> CloudflareResponse<T> {
    fn into_result(self) -> Result<T> {
        if self.success {
            self.result
                .ok_or_else(|| anyhow!("Cloudflare API response was successful but empty"))
        } else {
            let message = self
                .errors
                .into_iter()
                .map(|error| format!("{}: {}", error.code, error.message))
                .collect::<Vec<_>>()
                .join("; ");
            Err(anyhow!("Cloudflare API error: {message}"))
        }
    }
}

#[derive(Debug, Deserialize)]
struct CloudflareError {
    code: u64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct CloudflareDnsRecord {
    id: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct CloudflareDnsRecordPayload<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    name: &'a str,
    content: String,
    ttl: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxied: Option<bool>,
}
