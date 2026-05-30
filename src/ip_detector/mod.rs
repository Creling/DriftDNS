pub mod providers;

use std::{
    net::IpAddr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::{
    config::{IpDetectorListConfig, RecordType},
    logger,
};
use providers::{
    cloudflare_dns::CloudflareDnsProvider, icanhazip::IcanhazipProvider, ip_sb::IpSbProvider,
    ipify::IpifyProvider,
};

#[async_trait]
pub trait IpDetector: Send + Sync {
    fn name(&self) -> &'static str;

    fn supported_record_types(&self) -> &'static [RecordType];

    fn supports(&self, record_type: RecordType) -> bool {
        self.supported_record_types().contains(&record_type)
    }

    async fn fetch_ip(&self, record_type: RecordType) -> Result<IpAddr>;
}

#[derive(Debug, Clone, Copy)]
pub struct DetectedPublicIp {
    pub detector_name: &'static str,
    pub ip: IpAddr,
}

pub fn build_detectors(config: &IpDetectorListConfig) -> Result<Vec<Box<dyn IpDetector>>> {
    config
        .provider_names()?
        .into_iter()
        .map(build_detector)
        .collect()
}

pub async fn fetch_ip_with_fallback(
    config: &IpDetectorListConfig,
    record_type: RecordType,
) -> Result<DetectedPublicIp> {
    let detectors = build_detectors(config)?;
    let mut failures = Vec::new();

    for index in random_order(detectors.len()) {
        let detector = &detectors[index];
        logger::info(
            "ip_detector",
            format!(
                "detector_try record_type={} detector={}",
                record_type.as_dns_type(),
                detector.name()
            ),
        );

        match detector.fetch_ip(record_type).await {
            Ok(ip) => {
                return Ok(DetectedPublicIp {
                    detector_name: detector.name(),
                    ip,
                });
            }
            Err(error) => {
                logger::warn(
                    "ip_detector",
                    format!(
                        "detector_failed record_type={} detector={} error={error:#}",
                        record_type.as_dns_type(),
                        detector.name()
                    ),
                );
                failures.push(format!("{}: {error:#}", detector.name()));
            }
        }
    }

    Err(anyhow!(
        "all IP detectors failed for {}: {}",
        record_type.as_dns_type(),
        failures.join("; ")
    ))
}

fn build_detector(name: &str) -> Result<Box<dyn IpDetector>> {
    let normalized = normalize_provider_name(name);
    match normalized.as_str() {
        "ipify" => Ok(Box::new(IpifyProvider::new())),
        "icanhazip" => Ok(Box::new(IcanhazipProvider::new())),
        "ip_sb" => Ok(Box::new(IpSbProvider::new())),
        "cloudflare_dns" => Ok(Box::new(CloudflareDnsProvider::new())),
        _ => Err(anyhow!("unsupported IP detector: {name}")),
    }
}

pub fn ensure_record_type_matches_ip(record_type: RecordType, ip: IpAddr) -> Result<IpAddr> {
    if record_type.matches_ip(ip) {
        Ok(ip)
    } else {
        Err(anyhow!(
            "{} provider returned {}, which does not match {}",
            record_type.as_dns_type(),
            ip,
            record_type.as_dns_type()
        ))
    }
}

fn normalize_provider_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['.', '-'], "_")
}

fn random_order(len: usize) -> Vec<usize> {
    let mut order = (0..len).collect::<Vec<_>>();
    let mut state = random_seed();

    for index in (1..order.len()).rev() {
        state = next_random(state);
        let swap_with = (state as usize) % (index + 1);
        order.swap(index, swap_with);
    }

    order
}

fn random_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

fn next_random(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}
