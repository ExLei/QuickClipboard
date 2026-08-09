use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

const DISCOVERY_PORT: u16 = 35692;
const DISCOVERY_PROTOCOL: &str = "quickclipboard-sync-transfer-lan-discovery";
const PACKET_LIMIT: usize = 2048;

static RESPONDER: Lazy<tokio::sync::Mutex<Option<ResponderState>>> = Lazy::new(|| tokio::sync::Mutex::new(None));

struct ResponderState {
    http_port: u16,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct DiscoveryPacket {
    protocol: String,
    kind: String,
    device_id: String,
    device_name: String,
    http_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredLanPeer {
    pub device_id: String,
    pub device_name: String,
    pub base_url: String,
    pub last_seen_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanLocalEndpoint {
    pub ip: String,
    pub base_url: String,
}

pub fn is_running() -> bool {
    if let Ok(state) = RESPONDER.try_lock() {
        return state
            .as_ref()
            .map(|state| !state.task.is_finished())
            .unwrap_or(false);
    }
    false
}

pub async fn start_responder(http_port: u16) -> Result<(), String> {
    let mut state = RESPONDER.lock().await;
    if let Some(existing) = state.as_ref() {
        if existing.http_port == http_port && !existing.task.is_finished() {
            return Ok(());
        }
    }
    if let Some(existing) = state.take() {
        existing.task.abort();
    }

    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))
        .await
        .map_err(|e| format!("局域网发现服务启动失败: {}", e))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("启用局域网发现广播失败: {}", e))?;

    let task = tokio::spawn(async move {
        let mut buffer = vec![0u8; PACKET_LIMIT];
        loop {
            let Ok((len, remote_addr)) = socket.recv_from(&mut buffer).await else {
                break;
            };
            if !is_valid_request(&buffer[..len]) {
                continue;
            }
            let response = response_packet(http_port);
            let Ok(bytes) = serde_json::to_vec(&response) else {
                continue;
            };
            let _ = socket.send_to(&bytes, remote_addr).await;
        }
    });

    *state = Some(ResponderState { http_port, task });
    Ok(())
}

pub async fn stop_responder() {
    let mut state = RESPONDER.lock().await;
    if let Some(state) = state.take() {
        state.task.abort();
    }
}

pub async fn discover(timeout_ms: u64) -> Result<Vec<DiscoveredLanPeer>, String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .map_err(|e| format!("启动局域网设备发现失败: {}", e))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("启用局域网发现广播失败: {}", e))?;

    let request = request_packet();
    let bytes = serde_json::to_vec(&request).map_err(|e| format!("序列化局域网发现请求失败: {}", e))?;
    for target in discovery_targets() {
        let _ = socket.send_to(&bytes, target).await;
    }

    let timeout_ms = timeout_ms.clamp(300, 5_000);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let own_device_id = super::runtime::device_id();
    let mut peers = HashMap::<String, DiscoveredLanPeer>::new();
    let mut buffer = vec![0u8; PACKET_LIMIT];

    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            break;
        };
        let receive = tokio::time::timeout(remaining, socket.recv_from(&mut buffer)).await;
        let Ok(Ok((len, remote_addr))) = receive else {
            break;
        };
        let Ok(packet) = serde_json::from_slice::<DiscoveryPacket>(&buffer[..len]) else {
            continue;
        };
        if packet.protocol != DISCOVERY_PROTOCOL || packet.kind != "response" || packet.device_id == own_device_id {
            continue;
        }
        peers.insert(
            packet.device_id.clone(),
            DiscoveredLanPeer {
                device_id: packet.device_id,
                device_name: packet.device_name,
                base_url: format!("http://{}:{}", remote_addr.ip(), packet.http_port),
                last_seen_at_ms: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    let mut peers = peers.into_values().collect::<Vec<_>>();
    peers.sort_by(|a, b| a.device_name.cmp(&b.device_name).then(a.device_id.cmp(&b.device_id)));
    Ok(peers)
}

pub fn local_endpoints(http_port: u16) -> Vec<LanLocalEndpoint> {
    let mut endpoints = Vec::new();
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for iface in addrs {
            let if_addrs::IfAddr::V4(addr) = iface.addr else {
                continue;
            };
            if addr.ip.is_loopback() {
                continue;
            }
            let ip = addr.ip.to_string();
            endpoints.push(LanLocalEndpoint {
                base_url: format!("http://{}:{}", ip, http_port),
                ip,
            });
        }
    }
    endpoints.sort_by(|a, b| a.ip.cmp(&b.ip));
    endpoints.dedup_by(|a, b| a.ip == b.ip);
    endpoints
}

fn is_valid_request(bytes: &[u8]) -> bool {
    let Ok(packet) = serde_json::from_slice::<DiscoveryPacket>(bytes) else {
        return false;
    };
    packet.protocol == DISCOVERY_PROTOCOL
        && packet.kind == "request"
        && packet.device_id != super::runtime::device_id()
}

fn request_packet() -> DiscoveryPacket {
    DiscoveryPacket {
        protocol: DISCOVERY_PROTOCOL.to_string(),
        kind: "request".to_string(),
        device_id: super::runtime::device_id(),
        device_name: super::runtime::device_name(),
        http_port: 0,
    }
}

fn response_packet(http_port: u16) -> DiscoveryPacket {
    let _ = super::runtime::current_pairing_code();
    DiscoveryPacket {
        protocol: DISCOVERY_PROTOCOL.to_string(),
        kind: "response".to_string(),
        device_id: super::runtime::device_id(),
        device_name: super::runtime::device_name(),
        http_port,
    }
}

fn discovery_targets() -> Vec<SocketAddr> {
    let mut targets = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT)];
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for iface in addrs {
            let if_addrs::IfAddr::V4(addr) = iface.addr else {
                continue;
            };
            let broadcast = ipv4_broadcast(addr.ip, addr.netmask);
            if !targets.iter().any(|target| target.ip() == IpAddr::V4(broadcast)) {
                targets.push(SocketAddr::new(IpAddr::V4(broadcast), DISCOVERY_PORT));
            }
        }
    }
    targets
}

fn ipv4_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    let ip = u32::from(ip);
    let netmask = u32::from(netmask);
    Ipv4Addr::from(ip | !netmask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_broadcast_computes_subnet_broadcast() {
        let ip: Ipv4Addr = "192.168.1.5".parse().unwrap();
        let mask: Ipv4Addr = "255.255.255.0".parse().unwrap();
        let expected: Ipv4Addr = "192.168.1.255".parse().unwrap();
        assert_eq!(ipv4_broadcast(ip, mask), expected);

        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let mask: Ipv4Addr = "255.0.0.0".parse().unwrap();
        let expected: Ipv4Addr = "10.255.255.255".parse().unwrap();
        assert_eq!(ipv4_broadcast(ip, mask), expected);

        let ip: Ipv4Addr = "172.16.0.1".parse().unwrap();
        let mask: Ipv4Addr = "255.240.0.0".parse().unwrap();
        let expected: Ipv4Addr = "172.31.255.255".parse().unwrap();
        assert_eq!(ipv4_broadcast(ip, mask), expected);
    }

    #[test]
    fn discovery_targets_always_starts_with_global_broadcast() {
        let targets = discovery_targets();
        assert!(!targets.is_empty());
        assert_eq!(targets[0], SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT));
        assert_eq!(targets[0].port(), 35692);
        assert!(targets.iter().all(|t| t.port() == DISCOVERY_PORT));
    }

    #[test]
    fn discovery_targets_are_deduplicated_ipv4_broadcasts() {
        let targets = discovery_targets();
        let mut ips = targets.iter().map(|t| t.ip()).collect::<Vec<_>>();
        ips.sort();
        ips.dedup();
        assert_eq!(ips.len(), targets.len(), "无重复目标");
        assert!(targets.iter().all(|t| t.is_ipv4()));
    }

    fn valid_request(device_id: &str) -> DiscoveryPacket {
        DiscoveryPacket {
            protocol: DISCOVERY_PROTOCOL.to_string(),
            kind: "request".to_string(),
            device_id: device_id.to_string(),
            device_name: "peer".to_string(),
            http_port: 0,
        }
    }

    #[test]
    fn is_valid_request_accepts_protocol_request_from_other_device() {
        let packet = valid_request("some-other-device");
        let bytes = serde_json::to_vec(&packet).unwrap();
        assert!(is_valid_request(&bytes));
    }

    #[test]
    fn is_valid_request_rejects_wrong_protocol_kind_and_self() {
        let mut packet = valid_request("other");
        packet.protocol = "other-protocol".to_string();
        assert!(!is_valid_request(&serde_json::to_vec(&packet).unwrap()));

        let mut packet = valid_request("other");
        packet.kind = "response".to_string();
        assert!(!is_valid_request(&serde_json::to_vec(&packet).unwrap()));

        // 本机自己的 device_id → 忽略
        let packet = valid_request(&super::super::runtime::device_id());
        assert!(!is_valid_request(&serde_json::to_vec(&packet).unwrap()));
    }

    #[test]
    fn is_valid_request_rejects_garbage() {
        assert!(!is_valid_request(b"not json"));
        assert!(!is_valid_request(b""));
    }

    #[test]
    fn response_packet_carries_http_port_and_identity() {
        let _guard = super::super::runtime::tests::PAIRING_TEST_LOCK.lock();
        let packet = response_packet(12345);
        assert_eq!(packet.protocol, "quickclipboard-sync-transfer-lan-discovery");
        assert_eq!(packet.kind, "response");
        assert_eq!(packet.http_port, 12345);
        // device_id 形状：UUID v4（36 字符、版本位为 '4'、全 hexdigit 或 '-'）
        let id = &packet.device_id;
        assert_eq!(id.len(), 36, "device_id 必须是 UUID 形状");
        assert_eq!(&id[14..15], "4", "UUID v4 版本位");
        assert!(
            id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "device_id 只能含十六进制字符与连字符"
        );
        assert!(!packet.device_name.is_empty());
    }

    #[test]
    fn local_endpoints_are_sorted_deduped_non_loopback_with_port() {
        let endpoints = local_endpoints(35691);
        let mut prev: Option<&str> = None;
        for endpoint in &endpoints {
            assert!(!endpoint.ip.contains("127."), "环回地址被排除: {}", endpoint.ip);
            assert_eq!(endpoint.base_url, format!("http://{}:35691", endpoint.ip));
            if let Some(prev_ip) = prev {
                assert!(prev_ip < endpoint.ip.as_str(), "按 ip 排序");
            }
            prev = Some(&endpoint.ip);
        }
    }

    #[test]
    fn responder_not_running_by_default() {
        assert!(!is_running());
    }
}
