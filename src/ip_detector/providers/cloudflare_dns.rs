use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tokio::{net::UdpSocket, time};

use crate::{config::RecordType, ip_detector::ensure_record_type_matches_ip};

use super::super::IpDetector;

const SUPPORTED_RECORD_TYPES: &[RecordType] = &[RecordType::A];
const SERVER: &str = "1.1.1.1:53";
const QUERY_NAME: &str = "whoami.cloudflare.";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const DNS_TYPE_TXT: u16 = 16;
const DNS_CLASS_CH: u16 = 3;

pub struct CloudflareDnsProvider;

impl CloudflareDnsProvider {
    pub fn new() -> Self {
        Self
    }

    async fn query_txt_ch(&self) -> Result<Vec<String>> {
        let server = SERVER
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid Cloudflare DNS server address {SERVER}"))?;
        if !server.is_ipv4() {
            return Err(anyhow!(
                "cloudflare_dns currently requires an IPv4 DNS server address, got {}",
                SERVER
            ));
        }

        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .context("failed to bind IPv4 UDP socket")?;
        socket
            .connect(server)
            .await
            .with_context(|| format!("failed to connect UDP socket to {server}"))?;

        let query_id = query_id();
        let request_bytes = build_txt_ch_query(query_id, QUERY_NAME)?;
        socket
            .send(&request_bytes)
            .await
            .with_context(|| format!("failed to send DNS query to {server}"))?;

        let mut response_bytes = vec![0; 512];
        let len = time::timeout(TIMEOUT, socket.recv(&mut response_bytes))
            .await
            .with_context(|| format!("timed out waiting for DNS response from {server}"))?
            .with_context(|| format!("failed to read DNS response from {server}"))?;

        let answers = extract_txt_answers(query_id, &response_bytes[..len])
            .context("failed to decode DNS response")?;

        if answers.is_empty() {
            Err(anyhow!(
                "DNS response for {} did not contain a TXT answer",
                QUERY_NAME
            ))
        } else {
            Ok(answers)
        }
    }
}

#[async_trait]
impl IpDetector for CloudflareDnsProvider {
    fn name(&self) -> &'static str {
        "cloudflare_dns"
    }

    fn supported_record_types(&self) -> &'static [RecordType] {
        SUPPORTED_RECORD_TYPES
    }

    async fn fetch_ip(&self, record_type: RecordType) -> Result<IpAddr> {
        if record_type != RecordType::A {
            return Err(anyhow!(
                "cloudflare_dns implements the IPv4 equivalent of `dig -4 TXT CH +short whoami.cloudflare @one.one.one.one`; use it for A records"
            ));
        }

        let answers = self.query_txt_ch().await?;
        let ip = answers
            .iter()
            .find_map(|answer| IpAddr::from_str(answer).ok())
            .ok_or_else(|| {
                anyhow!(
                    "DNS TXT answers did not contain a valid IP address: {}",
                    answers.join(", ")
                )
            })?;

        ensure_record_type_matches_ip(record_type, ip)
    }
}

fn query_id() -> u16 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u16)
        .unwrap_or(0)
}

fn build_txt_ch_query(id: u16, query_name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());

    encode_qname(query_name, &mut bytes)?;
    bytes.extend_from_slice(&DNS_TYPE_TXT.to_be_bytes());
    bytes.extend_from_slice(&DNS_CLASS_CH.to_be_bytes());

    Ok(bytes)
}

fn encode_qname(query_name: &str, bytes: &mut Vec<u8>) -> Result<()> {
    let query_name = query_name.trim_end_matches('.');
    if query_name.is_empty() {
        return Err(anyhow!("DNS query name cannot be empty"));
    }

    for label in query_name.split('.') {
        if label.is_empty() {
            return Err(anyhow!("DNS query name contains an empty label"));
        }
        if label.len() > 63 {
            return Err(anyhow!("DNS label is too long in {query_name}"));
        }

        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.push(0);

    Ok(())
}

fn extract_txt_answers(expected_id: u16, bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.len() < 12 {
        return Err(anyhow!("DNS response is shorter than the header"));
    }

    let response_id = read_u16(bytes, 0)?;
    if response_id != expected_id {
        return Err(anyhow!(
            "DNS response id mismatch: expected {expected_id}, got {response_id}"
        ));
    }

    let flags = read_u16(bytes, 2)?;
    if flags & 0x8000 == 0 {
        return Err(anyhow!("DNS response is not marked as a response"));
    }

    let rcode = flags & 0x000f;
    if rcode != 0 {
        return Err(anyhow!("DNS server returned rcode {rcode}"));
    }

    let question_count = read_u16(bytes, 4)? as usize;
    let answer_count = read_u16(bytes, 6)? as usize;

    let mut offset = 12;
    for _ in 0..question_count {
        offset = skip_name(bytes, offset)?;
        offset = checked_add(offset, 4, bytes.len(), "question fields")?;
    }

    let mut answers = Vec::new();
    for _ in 0..answer_count {
        offset = skip_name(bytes, offset)?;
        let record_type = read_u16(bytes, offset)?;
        let record_class = read_u16(bytes, offset + 2)?;
        let rdlength = read_u16(bytes, offset + 8)? as usize;
        offset = checked_add(offset, 10, bytes.len(), "answer header")?;
        let rdata_end = checked_add(offset, rdlength, bytes.len(), "answer data")?;

        if record_type == DNS_TYPE_TXT && record_class == DNS_CLASS_CH {
            let txt = parse_txt_rdata(&bytes[offset..rdata_end])?;
            if !txt.trim().is_empty() {
                answers.push(txt.trim().to_string());
            }
        }

        offset = rdata_end;
    }

    Ok(answers)
}

fn parse_txt_rdata(mut bytes: &[u8]) -> Result<String> {
    let mut value = String::new();
    while let Some((&length, rest)) = bytes.split_first() {
        let length = length as usize;
        if rest.len() < length {
            return Err(anyhow!("TXT record length exceeds available data"));
        }

        let (chunk, remaining) = rest.split_at(length);
        value.push_str(&String::from_utf8_lossy(chunk));
        bytes = remaining;
    }

    Ok(value)
}

fn skip_name(bytes: &[u8], mut offset: usize) -> Result<usize> {
    let mut jumped = false;
    let mut jumps = 0;
    let mut next_offset = offset;

    loop {
        if offset >= bytes.len() {
            return Err(anyhow!("DNS name exceeds response length"));
        }

        let length = bytes[offset];
        if length & 0xc0 == 0xc0 {
            if offset + 1 >= bytes.len() {
                return Err(anyhow!("compressed DNS name pointer is incomplete"));
            }
            if !jumped {
                next_offset = offset + 2;
            }
            let pointer = (((length as usize) & 0x3f) << 8) | bytes[offset + 1] as usize;
            offset = pointer;
            jumped = true;
            jumps += 1;
            if jumps > 16 {
                return Err(anyhow!("too many DNS name compression jumps"));
            }
            continue;
        }

        if length & 0xc0 != 0 {
            return Err(anyhow!("unsupported DNS name label format"));
        }

        offset += 1;
        if length == 0 {
            return Ok(if jumped { next_offset } else { offset });
        }

        offset = checked_add(offset, length as usize, bytes.len(), "DNS name label")?;
        if !jumped {
            next_offset = offset;
        }
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = checked_add(offset, 2, bytes.len(), "u16")?;
    Ok(u16::from_be_bytes([bytes[offset], bytes[end - 1]]))
}

fn checked_add(offset: usize, len: usize, max: usize, context: &str) -> Result<usize> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| anyhow!("DNS {context} offset overflow"))?;
    if end > max {
        Err(anyhow!("DNS {context} exceeds response length"))
    } else {
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::{DNS_CLASS_CH, DNS_TYPE_TXT, build_txt_ch_query, extract_txt_answers};

    #[test]
    fn builds_txt_ch_query() {
        let query = build_txt_ch_query(0x1234, "whoami.cloudflare.").unwrap();

        assert_eq!(&query[0..2], &[0x12, 0x34]);
        assert_eq!(&query[12..], b"\x06whoami\x0acloudflare\0\0\x10\0\x03");
    }

    #[test]
    fn extracts_txt_answer_from_compressed_response() {
        let mut response = build_txt_ch_query(0x1234, "whoami.cloudflare.").unwrap();
        response[2] = 0x80;
        response[7] = 1;
        response.extend_from_slice(&[0xc0, 0x0c]);
        response.extend_from_slice(&DNS_TYPE_TXT.to_be_bytes());
        response.extend_from_slice(&DNS_CLASS_CH.to_be_bytes());
        response.extend_from_slice(&0u32.to_be_bytes());
        response.extend_from_slice(&13u16.to_be_bytes());
        response.push(12);
        response.extend_from_slice(b"203.0.113.10");

        let answers = extract_txt_answers(0x1234, &response).unwrap();

        assert_eq!(answers, vec!["203.0.113.10"]);
    }
}
