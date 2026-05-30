mod config;
mod dns_backend;
mod history;
mod ip_detector;
mod logger;
mod web;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use tokio::time;

use crate::{
    config::{Config, IpDetectorListConfig, RecordConfig, RecordType},
    history::History,
};

#[derive(Debug, Parser)]
#[command(author, version, about = "DriftDNS dynamic DNS updater")]
struct Cli {
    #[arg(short, long, default_value = "ddns.yaml")]
    config: PathBuf,

    #[arg(
        long,
        help = "Print planned DNS changes without writing DNS or history"
    )]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let loaded = load_runtime_config(&cli.config).await?;

    if loaded.config.check_interval.is_some() || loaded.config.web.enabled {
        if loaded.config.web.enabled {
            web::spawn(cli.config.clone(), loaded.config.web.bind.clone());
        }
        run_forever(cli.config, cli.dry_run, loaded).await;
        Ok(())
    } else {
        run_update(&loaded.config, cli.dry_run).await
    }
}

struct LoadedConfig {
    config: Config,
    modified: Option<SystemTime>,
}

enum WaitEvent {
    Tick,
    ConfigChanged(Option<SystemTime>),
}

async fn run_forever(config_path: PathBuf, cli_dry_run: bool, mut loaded: LoadedConfig) {
    run_update_and_log(&loaded.config, cli_dry_run).await;

    loop {
        let check_interval = loaded
            .config
            .check_interval
            .unwrap_or_else(|| Duration::from_secs(60));

        match wait_for_tick_or_config_change(&config_path, loaded.modified, check_interval).await {
            WaitEvent::Tick => {
                run_update_and_log(&loaded.config, cli_dry_run).await;
            }
            WaitEvent::ConfigChanged(modified) => {
                logger::info(
                    "main",
                    format!("config_changed path={}", config_path.display()),
                );
                match load_runtime_config(&config_path).await {
                    Ok(next_loaded) => {
                        loaded = next_loaded;
                        run_update_and_log(&loaded.config, cli_dry_run).await;
                    }
                    Err(error) => {
                        loaded.modified = modified;
                        logger::error("main", format!("config_reload_failed error={error:#}"));
                    }
                }
            }
        }
    }
}

async fn run_update_and_log(config: &Config, cli_dry_run: bool) {
    if let Err(error) = run_update(config, cli_dry_run).await {
        logger::error("main", format!("update_failed error={error:#}"));
    }
}

async fn load_runtime_config(path: &Path) -> Result<LoadedConfig> {
    let config = Config::load(path.to_path_buf()).await?;
    validate_ip_detector_placement(&config)?;
    let modified = config_modified(path).await?;

    Ok(LoadedConfig { config, modified })
}

async fn wait_for_tick_or_config_change(
    config_path: &Path,
    last_modified: Option<SystemTime>,
    check_interval: Duration,
) -> WaitEvent {
    let tick = time::sleep(check_interval);
    tokio::pin!(tick);

    let mut config_poll = time::interval(Duration::from_secs(1));
    config_poll.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut tick => return WaitEvent::Tick,
            _ = config_poll.tick() => {
                match config_modified(config_path).await {
                    Ok(modified) if modified != last_modified => {
                        return WaitEvent::ConfigChanged(modified);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        logger::warn(
                            "main",
                            format!(
                                "config_inspect_failed path={} error={error:#}",
                                config_path.display()
                            ),
                        );
                    }
                }
            }
        }
    }
}

async fn config_modified(path: &Path) -> Result<Option<SystemTime>> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to inspect config file {}", path.display()))?;
    Ok(metadata.modified().ok())
}

async fn run_update(config: &Config, cli_dry_run: bool) -> Result<()> {
    let dry_run = cli_dry_run || config.dry_run;
    let mut history = History::load(config.history_file.clone()).await?;
    let record_types = config
        .records
        .iter()
        .map(|record| record.record_type)
        .collect::<BTreeSet<_>>();

    logger::info("main", format!("update_started dry_run={dry_run}"));

    let mut failures = Vec::new();
    let mut history_changed = false;
    for record_type in record_types {
        match update_record_type(config, &mut history, dry_run, record_type).await {
            Ok(updated_history) => {
                history_changed |= updated_history;
            }
            Err(error) => {
                failures.push(format!("{}: {error:#}", record_type.as_dns_type()));
            }
        }
    }

    if !dry_run && history_changed {
        history.save().await?;
    }

    if failures.is_empty() {
        logger::info("main", "update_finished status=ok");
        Ok(())
    } else {
        Err(anyhow!(
            "update finished with {} failure(s): {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

async fn update_record_type(
    config: &Config,
    history: &mut History,
    dry_run: bool,
    record_type: RecordType,
) -> Result<bool> {
    let ip_detector_config = ip_detector_config(config, record_type)?;
    let fetched = ip_detector::fetch_ip_with_fallback(ip_detector_config, record_type)
        .await
        .with_context(|| format!("failed to fetch public {}", record_type.as_dns_type()))?;
    let current_ip = fetched.ip;
    logger::info(
        "main",
        format!(
            "ip_detected record_type={} detector={} ip={}",
            record_type.as_dns_type(),
            fetched.detector_name,
            current_ip
        ),
    );

    let history_key = record_type.history_key();
    let previous_ip = history.get(history_key);
    if previous_ip == Some(current_ip) {
        logger::info(
            "main",
            format!(
                "ip_unchanged record_type={} ip={}",
                record_type.as_dns_type(),
                current_ip
            ),
        );
        return Ok(false);
    }

    logger::info(
        "main",
        format!(
            "ip_changed record_type={} previous={} current={}",
            record_type.as_dns_type(),
            previous_ip
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "none".to_string()),
            current_ip
        ),
    );

    let mut failures = Vec::new();
    for record in config
        .records
        .iter()
        .filter(|record| record.record_type == record_type)
    {
        if let Err(error) = upsert_record(record, current_ip, dry_run).await {
            failures.push(format!(
                "{} {}: {error:#}",
                record.record_type.as_dns_type(),
                record.name
            ));
        }
    }

    if !failures.is_empty() {
        return Err(anyhow!(
            "record updates failed; history was not advanced: {}",
            failures.join("; ")
        ));
    }

    if dry_run {
        Ok(false)
    } else {
        history.set(record_type.history_key(), current_ip, config.history_limit);
        Ok(true)
    }
}

async fn upsert_record(
    record: &RecordConfig,
    current_ip: std::net::IpAddr,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        logger::info(
            "main",
            format!(
                "dns_upsert_planned record_type={} name={} backend={} ip={}",
                record.record_type.as_dns_type(),
                record.name,
                record.backend.backend_name(),
                current_ip
            ),
        );
        return Ok(());
    }

    let dns_backend = dns_backend::build_backend(&record.backend)?;
    logger::info(
        "main",
        format!(
            "dns_upsert_started record_type={} name={} backend={} ip={}",
            record.record_type.as_dns_type(),
            record.name,
            dns_backend.name(),
            current_ip
        ),
    );
    dns_backend.upsert_record(record, current_ip).await?;
    logger::info(
        "main",
        format!(
            "dns_upsert_finished record_type={} name={} ip={}",
            record.record_type.as_dns_type(),
            record.name,
            current_ip
        ),
    );

    Ok(())
}

fn ip_detector_config(config: &Config, record_type: RecordType) -> Result<&IpDetectorListConfig> {
    match record_type {
        RecordType::A => config
            .ip_detector
            .ipv4
            .as_ref()
            .context("ip_detector.ipv4 is not configured"),
        RecordType::Aaaa => config
            .ip_detector
            .ipv6
            .as_ref()
            .context("ip_detector.ipv6 is not configured"),
    }
}

fn validate_ip_detector_placement(config: &Config) -> Result<()> {
    if let Some(provider_config) = &config.ip_detector.ipv4 {
        ensure_ip_detector_supports(provider_config, RecordType::A)?;
    }

    if let Some(provider_config) = &config.ip_detector.ipv6 {
        ensure_ip_detector_supports(provider_config, RecordType::Aaaa)?;
    }

    Ok(())
}

fn ensure_ip_detector_supports(
    provider_config: &IpDetectorListConfig,
    record_type: RecordType,
) -> Result<()> {
    for provider in ip_detector::build_detectors(provider_config)? {
        if provider.supports(record_type) {
            continue;
        }

        let supported = provider
            .supported_record_types()
            .iter()
            .map(|record_type| record_type.as_dns_type())
            .collect::<Vec<_>>()
            .join("/");

        return Err(anyhow!(
            "ip_detector.{} uses detector {}, but it only supports {}",
            match record_type {
                RecordType::A => "ipv4",
                RecordType::Aaaa => "ipv6",
            },
            provider.name(),
            supported
        ));
    }

    Ok(())
}
