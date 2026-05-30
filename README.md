# DriftDNS

Lightweight dynamic DNS updater with a built-in web dashboard. Keeps your `A` and `AAAA` records in sync with your current public IP.

<img width="2468" height="1352" alt="image" src="https://github.com/user-attachments/assets/fce9cebb-ea9e-40f4-b52e-4eb050086358" />

## Features

- Detects public IPv4 and IPv6 using multiple fallback providers
- Updates Cloudflare DNS only when your IP actually changes *(additional DNS backends may be added on request)*
- Maintains a local IP history log
- Auto-reloads config on file changes
- Built-in dashboard and JSON status endpoint

## How It Works

1. Detects your current public IP for each configured record type.
2. Compares it against the last known IP in the history file.
3. If changed, updates all matching DNS records of that type.
4. History only advances after all updates for that type succeed.

History is tracked **per record type**: all `A` records share one IPv4 history, all `AAAA` records share one IPv6 history.

## Supported Providers

### IP Detectors

- `ipify` — IPv4 & IPv6
- `icanhazip` — IPv4 & IPv6
- `ip.sb` — IPv4 & IPv6
- `cloudflare_dns` — IPv4 only

### DNS Backends

- `cloudflare` **(additional backends may be added on request)**

## Configuration

Start from the example file:

```bash
cp ddns.example.yaml ddns.yaml
```

```yaml
history_file: ddns-history.txt
dry_run: false
check_interval: 1m
history_limit: 10

web:
  enabled: true
  bind: 127.0.0.1:8080

ip_detector:
  ipv4:
    provider: cloudflare_dns, ipify, ip.sb
  ipv6:
    provider: icanhazip, ip.sb, ipify

records:
  - name: example.com
    type: A
    backend:
      provider: cloudflare
      api_token_env: CLOUDFLARE_API_TOKEN
      # api_token: your_token
      zone_id: your_zone_id
      ttl: 1
      proxied: false

  - name: example.com
    type: AAAA
    backend:
      provider: cloudflare
      api_token_env: CLOUDFLARE_API_TOKEN
      # api_token: your_token
      zone_id: your_zone_id
      ttl: 1
      proxied: false
```

### Key Options

| Option | Description |
|--------|-------------|
| `history_file` | Path to local IP history file |
| `dry_run` | Simulate without making changes |
| `check_interval` | Check frequency (`30s`, `5m`, `1h`, `1d`) |
| `history_limit` | Max history entries per record type |
| `web.enabled` | Toggle built-in dashboard |
| `web.bind` | Dashboard listen address (e.g. `0.0.0.0:8080`) |

### IP Detector

Provider lists are comma-separated and tried in **random order** — if one fails, the next is used until one succeeds or all fail. Names are normalized (`ip.sb`, `ip-sb`, `ip_sb` all accepted).

### DNS Backend

#### Cloudflare

API token can be set directly via `api_token`, or by referencing an environment variable with `api_token_env`. If both are set, `api_token` takes precedence.

## Running

### Local

```bash
cargo run -- --config ddns.yaml
```

### Docker

```bash
# Build
docker build -t driftdns .

# Or pull from GHCR
docker pull ghcr.io/creling/driftdns:sha-a10d75d

# Run
docker run \
  -v "<host path>/ddns.yaml:/config/ddns.yaml:ro" \
  -v "<host path>/data:/data" \
  driftdns
```
