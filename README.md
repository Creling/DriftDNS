# DriftDNS

Rust CLI for keeping DNS `A` and `AAAA` records when the detected public IP changes.

## Behavior

1. Detect the current public IP with one or more IP detectors.
2. Compare it with the last recorded IP in the history file.
3. Update matching DNS records through the configured DNS backend.
4. Advance the history file only after all records for that IP type are updated successfully.

## IP Detectors

- `ipify` (`A`, `AAAA`)
- `icanhazip` (`A`, `AAAA`)
- `ip.sb` (`A`, `AAAA`)
- `cloudflare_dns` (`A` only)

`cloudflare_dns` performs a direct DNS UDP query and does not execute `dig`.

Equivalent command:

```bash
dig -4 TXT CH +short whoami.cloudflare @one.one.one.one
```

## DNS Backends

- `cloudflare`

## Configuration

Create a local config:

```bash
cp ddns.example.yaml ddns.yaml
```

Example:

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
      zone_id: your_cloudflare_zone_id
      ttl: 1
      proxied: false
```

`ip_detector.*.provider` accepts a comma-separated detector list. Detectors are tried in random order; failed detectors are skipped until all candidates fail.

IP detector endpoints are fixed in code. Custom URLs, DNS servers, query names, and timeouts are not supported in the config file.

`records[].backend` selects the DNS backend for each record.

`history_limit` controls how many historical IP entries are retained for each record type.

`check_interval` is optional. Supported units are `s`, `m`, `h`, and `d`, for example `30s`, `5m`, `1h`, `1d`.

When `check_interval` is set:

- The first update runs immediately.
- Updates continue at the configured interval.
- The config file is watched by modified time and reloaded when changed.
- Update or reload errors are logged and the process keeps running.

When `check_interval` is not set, the program runs once and exits.

When `web.enabled` is true, DriftDNS serves a dashboard at the configured `bind` address. The process stays alive even if `check_interval` is not set. The dashboard is available at `/`; the raw state is available at `/api/state`.

## Cloudflare

Use an API token with DNS edit access for the target zone:

```bash
export CLOUDFLARE_API_TOKEN=your_token
```

The token can also be set per record with `api_token`, or loaded from a custom environment variable with `api_token_env`.

## Run

```bash
cargo run -- --config ddns.yaml
```

Dry run:

```bash
cargo run -- --config ddns.yaml --dry-run
```

## Logging

Logs are written to stderr:

```text
1780076387.623 level=INFO target=main update_started dry_run=true
```

## History File

The history file is plain text:

```text
A|1710000000|203.0.113.10
AAAA|1710000300|2001:db8::10
```
