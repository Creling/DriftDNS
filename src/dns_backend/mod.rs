pub mod providers;

use std::net::IpAddr;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::{DnsBackendConfig, RecordConfig};
use providers::cloudflare::CloudflareBackend;

#[async_trait]
pub trait DnsBackend: Send + Sync {
    fn name(&self) -> &'static str;

    async fn upsert_record(&self, record: &RecordConfig, ip: IpAddr) -> Result<()>;
}

pub fn build_backend(config: &DnsBackendConfig) -> Result<Box<dyn DnsBackend>> {
    match config {
        DnsBackendConfig::Cloudflare(config) => Ok(Box::new(CloudflareBackend::new(config)?)),
    }
}
