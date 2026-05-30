use std::{net::IpAddr, str::FromStr};

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::{config::RecordType, ip_detector::ensure_record_type_matches_ip};

use super::super::IpDetector;

const SUPPORTED_RECORD_TYPES: &[RecordType] = &[RecordType::A, RecordType::Aaaa];

pub struct IpSbProvider {
    client: reqwest::Client,
}

impl IpSbProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, record_type: RecordType) -> &str {
        match record_type {
            RecordType::A => "https://api-ipv4.ip.sb/ip",
            RecordType::Aaaa => "https://api-ipv6.ip.sb/ip",
        }
    }
}

#[async_trait]
impl IpDetector for IpSbProvider {
    fn name(&self) -> &'static str {
        "ip.sb"
    }

    fn supported_record_types(&self) -> &'static [RecordType] {
        SUPPORTED_RECORD_TYPES
    }

    async fn fetch_ip(&self, record_type: RecordType) -> Result<IpAddr> {
        let url = self.url(record_type);
        let body = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to call {url}"))?
            .error_for_status()
            .with_context(|| format!("{url} returned an error status"))?
            .text()
            .await
            .with_context(|| format!("failed to read response from {url}"))?;

        let ip = IpAddr::from_str(body.trim())
            .with_context(|| format!("{url} did not return a valid IP address"))?;
        ensure_record_type_matches_ip(record_type, ip)
    }
}
