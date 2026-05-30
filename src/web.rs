use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{
    config::{Config, RecordConfig},
    history::{History, HistoryEntry},
    logger,
};

pub fn spawn(config_path: PathBuf, bind: String) {
    tokio::spawn(async move {
        if let Err(error) = serve(config_path, &bind).await {
            logger::error("web", format!("server_stopped bind={bind} error={error:#}"));
        }
    });
}

async fn serve(config_path: PathBuf, bind: &str) -> Result<()> {
    let addr = bind
        .parse::<SocketAddr>()
        .with_context(|| format!("invalid web bind address {bind}"))?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind web dashboard to {addr}"))?;
    logger::info("web", format!("server_started bind={addr}"));

    loop {
        let (stream, peer) = listener.accept().await?;
        let config_path = config_path.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, config_path).await {
                logger::warn("web", format!("request_failed peer={peer} error={error:#}"));
            }
        });
    }
}

async fn handle_connection(mut stream: TcpStream, config_path: PathBuf) -> Result<()> {
    let mut buffer = vec![0; 4096];
    let len = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let response = match path {
        "/" => http_response("200 OK", "text/html; charset=utf-8", dashboard_html()),
        "/api/state" => match load_state(config_path).await {
            Ok(state) => http_response(
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_string_pretty(&state)?,
            ),
            Err(error) => http_response(
                "500 Internal Server Error",
                "application/json; charset=utf-8",
                serde_json::json!({ "error": format!("{error:#}") }).to_string(),
            ),
        },
        _ => http_response("404 Not Found", "text/plain; charset=utf-8", "not found"),
    };

    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn load_state(config_path: PathBuf) -> Result<DashboardState> {
    let config = Config::load(config_path).await?;
    let history = History::load(config.history_file.clone()).await?;
    let records = config
        .records
        .iter()
        .map(|record| record_state(record, &history, config.history_limit))
        .collect();

    Ok(DashboardState {
    check_interval: config.check_interval.map(format_duration),
        history_limit: config.history_limit,
        records,
    })
}

fn record_state(record: &RecordConfig, history: &History, history_limit: usize) -> RecordState {
    let history_key = record.record_type.history_key();
    let latest = history.latest_entry(history_key);
    RecordState {
        name: record.name.clone(),
        record_type: record.record_type.as_dns_type(),
        dns_backend: record.backend.backend_name(),
        current_ip: latest.map(|entry| entry.ip.to_string()),
        last_changed_at: latest.and_then(|entry| {
            if entry.changed_at == 0 {
                None
            } else {
                Some(entry.changed_at)
            }
        }),
        history_limit,
        history: history.entries(history_key).to_vec(),
    }
}

fn http_response(status: &str, content_type: &str, body: impl AsRef<str>) -> String {
    let body = body.as_ref();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[derive(Debug, Serialize)]
struct DashboardState {
  check_interval: Option<String>,
    history_limit: usize,
    records: Vec<RecordState>,
}

#[derive(Debug, Serialize)]
struct RecordState {
    name: String,
    record_type: &'static str,
    dns_backend: &'static str,
    current_ip: Option<String>,
    last_changed_at: Option<u64>,
    history_limit: usize,
    history: Vec<HistoryEntry>,
}

fn dashboard_html() -> &'static str {
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>DriftDNS</title>
  <style>
    :root {
      color-scheme: light;
      --bg: #f6f7f9;
      --panel: #ffffff;
      --text: #17202a;
      --muted: #64748b;
      --line: #d9dee7;
      --accent: #0f766e;
      --accent-soft: #d9f3ee;
      --warn: #9f580a;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      font-size: 14px;
    }
    header {
      border-bottom: 1px solid var(--line);
      background: var(--panel);
    }
    .wrap {
      width: min(1180px, calc(100vw - 32px));
      margin: 0 auto;
    }
    .topbar {
      min-height: 64px;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
    }
    h1 {
      margin: 0;
      font-size: 22px;
      font-weight: 700;
      letter-spacing: 0;
    }
    .status {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      color: var(--muted);
      white-space: nowrap;
    }
    .dot {
      width: 9px;
      height: 9px;
      border-radius: 50%;
      background: var(--accent);
    }
    main { padding: 24px 0 32px; }
    .summary {
      display: grid;
      grid-template-columns: repeat(4, minmax(0, 1fr));
      gap: 12px;
      margin-bottom: 18px;
    }
    .metric {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 14px 16px;
      min-width: 0;
    }
    .metric .label {
      color: var(--muted);
      font-size: 12px;
      text-transform: uppercase;
      letter-spacing: 0;
    }
    .metric .value {
      margin-top: 4px;
      font-size: 20px;
      font-weight: 700;
      overflow-wrap: anywhere;
    }
    .table-shell {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
      overflow: hidden;
    }
    table {
      width: 100%;
      table-layout: fixed;
      border-collapse: collapse;
    }
    col.record { width: 17%; }
    col.type { width: 9%; }
    col.current-ip { width: 17%; }
    col.backend { width: 14%; }
    col.changed-at { width: 16%; }
    col.history { width: 27%; }
    th, td {
      text-align: left;
      padding: 12px 14px;
      border-bottom: 1px solid var(--line);
      vertical-align: top;
      overflow-wrap: anywhere;
    }
    th {
      background: #fbfcfe;
      color: var(--muted);
      font-size: 12px;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0;
    }
    tr:last-child td { border-bottom: 0; }
    code {
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      background: #eef2f7;
      padding: 2px 6px;
      border-radius: 5px;
      overflow-wrap: anywhere;
    }
    .pill {
      display: inline-flex;
      align-items: center;
      border-radius: 999px;
      padding: 3px 8px;
      background: var(--accent-soft);
      color: #075e56;
      font-weight: 700;
      font-size: 12px;
    }
    .muted { color: var(--muted); }
    .history {
      display: flex;
      flex-direction: column;
      gap: 5px;
      margin-top: 8px;
    }
    details.history-toggle {
      display: block;
      max-width: 100%;
    }
    details.history-toggle > summary {
      cursor: pointer;
      list-style: none;
      user-select: none;
      color: var(--accent);
      font-weight: 700;
    }
    details.history-toggle > summary {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    details.history-toggle > summary::-webkit-details-marker {
      display: none;
    }
    .history-row {
      display: flex;
      justify-content: space-between;
      gap: 12px;
      color: var(--muted);
    }
    .error {
      display: none;
      margin-bottom: 18px;
      border: 1px solid #f0c36d;
      background: #fff8e7;
      color: var(--warn);
      border-radius: 8px;
      padding: 12px 14px;
    }
    @media (max-width: 800px) {
      .summary { grid-template-columns: 1fr; }
      .topbar { align-items: flex-start; flex-direction: column; padding: 14px 0; }
      .table-shell { overflow-x: auto; }
      table { min-width: 780px; }
    }
  </style>
</head>
<body>
  <header>
    <div class="wrap topbar">
      <h1>DriftDNS</h1>
      <div class="status"><span class="dot"></span><span id="updated">Loading</span></div>
    </div>
  </header>
  <main class="wrap">
    <div id="error" class="error"></div>
    <section class="summary">
      <div class="metric"><div class="label">Records</div><div id="record-count" class="value">0</div></div>
      <div class="metric"><div class="label">DNS Backends</div><div id="backend-count" class="value">0</div></div>
      <div class="metric"><div class="label">Check Interval</div><div id="check-interval" class="value">disabled</div></div>
      <div class="metric"><div class="label">History Limit</div><div id="history-limit" class="value">0</div></div>
    </section>
    <section class="table-shell">
      <table>
        <colgroup>
          <col class="record">
          <col class="type">
          <col class="current-ip">
          <col class="backend">
          <col class="changed-at">
          <col class="history">
        </colgroup>
        <thead>
          <tr>
            <th>Record</th>
            <th>Type</th>
            <th>Current IP</th>
            <th>DNS Backend</th>
            <th>Last IP Change</th>
            <th>History</th>
          </tr>
        </thead>
        <tbody id="records"></tbody>
      </table>
    </section>
  </main>
  <script>
    const tbody = document.querySelector('#records');
    const errorBox = document.querySelector('#error');

    function escapeHtml(value) {
      return String(value ?? '').replace(/[&<>"']/g, ch => ({
        '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
      }[ch]));
    }

    function formatTime(seconds) {
      if (!seconds) return '<span class="muted">unknown</span>';
      return escapeHtml(new Date(seconds * 1000).toLocaleString());
    }

    function renderHistory(items) {
      if (!items.length) return '<span class="muted">none</span>';
      return `<details class="history-toggle">
        <summary>${items.length} entr${items.length === 1 ? 'y' : 'ies'}</summary>
        <div class="history">${items.map(item => `
          <div class="history-row">
            <code>${escapeHtml(item.ip)}</code>
            <span>${formatTime(item.changed_at)}</span>
          </div>
        `).join('')}</div>
      </details>`;
    }

    async function load() {
      try {
        const response = await fetch('/api/state', { cache: 'no-store' });
        const state = await response.json();
        if (!response.ok) throw new Error(state.error || 'request failed');

        errorBox.style.display = 'none';
        document.querySelector('#record-count').textContent = state.records.length;
        document.querySelector('#backend-count').textContent = new Set(state.records.map(record => record.dns_backend)).size;
        document.querySelector('#check-interval').textContent = state.check_interval || 'disabled';
        document.querySelector('#history-limit').textContent = state.history_limit;
        document.querySelector('#updated').textContent = `Updated ${new Date().toLocaleTimeString()}`;

        tbody.innerHTML = state.records.map(record => `
          <tr>
            <td><strong>${escapeHtml(record.name)}</strong></td>
            <td><span class="pill">${escapeHtml(record.record_type)}</span></td>
            <td>${record.current_ip ? `<code>${escapeHtml(record.current_ip)}</code>` : '<span class="muted">unknown</span>'}</td>
            <td>${escapeHtml(record.dns_backend)}</td>
            <td>${formatTime(record.last_changed_at)}</td>
            <td>${renderHistory(record.history || [])}</td>
          </tr>
        `).join('');
      } catch (error) {
        errorBox.textContent = error.message;
        errorBox.style.display = 'block';
        document.querySelector('#updated').textContent = 'Update failed';
      }
    }

    load();
    setInterval(load, 5000);
  </script>
</body>
</html>
"#
}

fn format_duration(duration: Duration) -> String {
  let seconds = duration.as_secs();
  if seconds == 0 {
    return "0s".to_string();
  }

  let days = seconds / 86_400;
  let hours = (seconds % 86_400) / 3_600;
  let minutes = (seconds % 3_600) / 60;
  let seconds = seconds % 60;

  let mut parts = Vec::new();
  if days > 0 {
    parts.push(format!("{days}d"));
  }
  if hours > 0 {
    parts.push(format!("{hours}h"));
  }
  if minutes > 0 {
    parts.push(format!("{minutes}m"));
  }
  if seconds > 0 || parts.is_empty() {
    parts.push(format!("{seconds}s"));
  }

  parts.join(" ")
}
