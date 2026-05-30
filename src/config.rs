use std::{net::IpAddr, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Deserializer, de};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_history_file")]
    pub history_file: PathBuf,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(
        default,
        alias = "interval",
        deserialize_with = "deserialize_optional_duration"
    )]
    pub check_interval: Option<Duration>,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default)]
    pub web: WebConfig,
    pub ip_detector: IpDetectorConfig,
    pub records: Vec<RecordConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_bind")]
    pub bind: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_web_bind(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpDetectorConfig {
    pub ipv4: Option<IpDetectorListConfig>,
    pub ipv6: Option<IpDetectorListConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IpDetectorListConfig {
    #[serde(alias = "providers")]
    pub provider: String,
}

impl IpDetectorListConfig {
    pub fn provider_names(&self) -> Result<Vec<&str>> {
        let names = self
            .provider
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();

        if names.is_empty() {
            Err(anyhow!("IP detector list cannot be empty"))
        } else {
            Ok(names)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum DnsBackendConfig {
    Cloudflare(CloudflareRecordConfig),
}

impl DnsBackendConfig {
    pub fn backend_name(&self) -> &'static str {
        match self {
            DnsBackendConfig::Cloudflare(_) => "cloudflare",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudflareRecordConfig {
    #[serde(default)]
    pub api_token: Option<String>,
    #[serde(default = "default_cloudflare_api_token_env")]
    pub api_token_env: Option<String>,
    pub zone_id: String,
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    #[serde(default)]
    pub proxied: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub record_type: RecordType,
    pub backend: DnsBackendConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RecordType {
    A,
    Aaaa,
}

impl RecordType {
    pub fn history_key(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
        }
    }

    pub fn as_dns_type(self) -> &'static str {
        self.history_key()
    }

    pub fn matches_ip(self, ip: IpAddr) -> bool {
        matches!(
            (self, ip),
            (RecordType::A, IpAddr::V4(_)) | (RecordType::Aaaa, IpAddr::V6(_))
        )
    }
}

impl FromStr for RecordType {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_uppercase().as_str() {
            "A" => Ok(RecordType::A),
            "AAAA" => Ok(RecordType::Aaaa),
            other => Err(anyhow!("unsupported DNS record type: {other}")),
        }
    }
}

impl Config {
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let raw = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config = serde_yaml::from_str::<Config>(&raw)
            .with_context(|| format!("failed to parse YAML config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.records.is_empty() {
            return Err(anyhow!("config must contain at least one DNS record"));
        }

        for record in &self.records {
            match record.record_type {
                RecordType::A if self.ip_detector.ipv4.is_none() => {
                    return Err(anyhow!(
                        "record {} is A, but ip_detector.ipv4 is not configured",
                        record.name
                    ));
                }
                RecordType::Aaaa if self.ip_detector.ipv6.is_none() => {
                    return Err(anyhow!(
                        "record {} is AAAA, but ip_detector.ipv6 is not configured",
                        record.name
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

fn default_history_file() -> PathBuf {
    PathBuf::from("ddns-history.txt")
}

fn default_history_limit() -> usize {
    10
}

fn default_web_bind() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_ttl() -> u32 {
    1
}

fn default_cloudflare_api_token_env() -> Option<String> {
    Some("CLOUDFLARE_API_TOKEN".to_string())
}

fn deserialize_optional_duration<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .as_deref()
        .map(parse_duration)
        .transpose()
        .map_err(de::Error::custom)
}

fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    if value.len() < 2 {
        return Err(anyhow!("duration must look like 1s, 1m, 1h, or 1d"));
    }

    let (amount, unit) = value.split_at(value.len() - 1);
    let amount = amount
        .parse::<u64>()
        .with_context(|| format!("invalid duration amount in {value}"))?;
    if amount == 0 {
        return Err(anyhow!("duration must be greater than zero"));
    }

    let seconds = match unit {
        "s" | "S" => amount,
        "m" | "M" => amount
            .checked_mul(60)
            .ok_or_else(|| anyhow!("duration is too large"))?,
        "h" | "H" => amount
            .checked_mul(60 * 60)
            .ok_or_else(|| anyhow!("duration is too large"))?,
        "d" | "D" => amount
            .checked_mul(24 * 60 * 60)
            .ok_or_else(|| anyhow!("duration is too large"))?,
        _ => {
            return Err(anyhow!(
                "unsupported duration unit in {value}; use s, m, h, or d"
            ));
        }
    };

    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        time::Duration,
    };

    use super::{Config, DnsBackendConfig, RecordType, parse_duration};

    #[test]
    fn parses_dns_record_types_from_yaml() {
        let a = serde_yaml::from_str::<RecordType>("A").unwrap();
        let aaaa = serde_yaml::from_str::<RecordType>("AAAA").unwrap();

        assert_eq!(a, RecordType::A);
        assert_eq!(aaaa, RecordType::Aaaa);
    }

    #[test]
    fn matches_ip_version_to_record_type() {
        assert!(RecordType::A.matches_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(RecordType::Aaaa.matches_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!RecordType::A.matches_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!RecordType::Aaaa.matches_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn parses_per_record_dns_backend() {
        let config = serde_yaml::from_str::<Config>(
            r#"
history_file: ddns-history.txt
ip_detector:
  ipv4:
    provider: ipify
records:
  - name: test.example.com
    type: A
    backend:
      provider: cloudflare
      zone_id: zone
      ttl: 60
      proxied: false
"#,
        )
        .unwrap();

        assert_eq!(config.records[0].name, "test.example.com");
        match &config.records[0].backend {
            DnsBackendConfig::Cloudflare(provider) => {
                assert_eq!(provider.zone_id, "zone");
                assert_eq!(provider.ttl, 60);
                assert_eq!(
                    provider.api_token_env.as_deref(),
                    Some("CLOUDFLARE_API_TOKEN")
                );
            }
        }
    }

    #[test]
    fn parses_comma_separated_ip_detectors() {
        let config = serde_yaml::from_str::<Config>(
            r#"
ip_detector:
  ipv4:
    provider: ipify, ip.sb
records:
  - name: test.example.com
    type: A
    backend:
      provider: cloudflare
      zone_id: zone
"#,
        )
        .unwrap();

        let provider_config = config.ip_detector.ipv4.unwrap();
        let names = provider_config.provider_names().unwrap();

        assert_eq!(names, vec!["ipify", "ip.sb"]);
    }

    #[test]
    fn rejects_ip_detector_custom_fields() {
        let error = serde_yaml::from_str::<Config>(
            r#"
ip_detector:
  ipv4:
    provider: ipify
    url: https://example.com
records:
  - name: test.example.com
    type: A
    backend:
      provider: cloudflare
      zone_id: zone
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `url`"));
    }

    #[test]
    fn parses_check_interval() {
        let config = serde_yaml::from_str::<Config>(
            r#"
check_interval: 1h
ip_detector:
  ipv4:
    provider: ipify
records:
  - name: test.example.com
    type: A
    backend:
      provider: cloudflare
      zone_id: zone
"#,
        )
        .unwrap();

        assert_eq!(config.check_interval.unwrap(), Duration::from_secs(60 * 60));
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(
            parse_duration("1d").unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
    }
}
