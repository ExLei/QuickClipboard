use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::pairing::{create_pairing_challenge, PairingChallenge};
use super::peer_store::PairedPeer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingCodeView {
    pub pairing_code: String,
    pub expires_at_ms: i64,
    pub remaining_attempts: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanRuntimeStatus {
    pub device_id: String,
    pub device_name: String,
    pub http_port: u16,
    pub http_running: bool,
    pub discovery_running: bool,
    pub local_endpoints: Vec<super::discovery::LanLocalEndpoint>,
    pub pairing_code: PairingCodeView,
    pub paired_count: usize,
}

#[derive(Debug, Clone)]
struct PairingState {
    challenge: PairingChallenge,
    failed_attempts: u8,
}

#[derive(Debug, Default)]
struct LanRuntime {
    pairing_state: Option<PairingState>,
}

static RUNTIME: Lazy<Mutex<LanRuntime>> = Lazy::new(|| Mutex::new(LanRuntime::default()));

pub fn status() -> LanRuntimeStatus {
    let pairing_code = current_pairing_code();
    let http_port = super::http_server::running_port().unwrap_or(super::DEFAULT_HTTP_PORT);
    LanRuntimeStatus {
        device_id: device_id(),
        device_name: device_name(),
        http_port,
        http_running: super::http_server::is_running(),
        discovery_running: super::discovery::is_running(),
        local_endpoints: super::discovery::local_endpoints(http_port),
        pairing_code,
        paired_count: super::peer_store::list_peers().len(),
    }
}

pub fn device_id() -> String {
    crate::services::sync_transfer::device_id()
}

pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "QuickClipboard Desktop".to_string())
}

pub fn current_pairing_code() -> PairingCodeView {
    let mut runtime = RUNTIME.lock();
    let state = ensure_pairing_state(&mut runtime);
    view_pairing_state(state)
}

pub fn refresh_pairing_code() -> PairingCodeView {
    let challenge = create_pairing_challenge();
    let state = PairingState {
        challenge,
        failed_attempts: 0,
    };
    let view = view_pairing_state(&state);
    RUNTIME.lock().pairing_state = Some(state);
    view
}

pub fn verify_pairing_code(pairing_code: &str) -> Result<(), String> {
    let mut runtime = RUNTIME.lock();
    let Some(state) = runtime.pairing_state.as_mut() else {
        runtime.pairing_state = Some(PairingState {
            challenge: create_pairing_challenge(),
            failed_attempts: 0,
        });
        return Err("配对码已刷新，请重新输入".to_string());
    };
    if is_expired(state.challenge.expires_at_ms) {
        runtime.pairing_state = Some(PairingState {
            challenge: create_pairing_challenge(),
            failed_attempts: 0,
        });
        return Err("配对码已过期，请重新输入".to_string());
    }
    if state.failed_attempts >= state.challenge.max_attempts {
        return Err("配对码尝试次数过多，请刷新后重试".to_string());
    }
    if state.challenge.pairing_code != pairing_code.trim() {
        state.failed_attempts = state.failed_attempts.saturating_add(1);
        return Err("配对码不正确".to_string());
    }
    state.failed_attempts = 0;
    Ok(())
}

pub fn confirm_pairing(device_id: String, device_name: String, base_url: String, pairing_code: String) -> Result<String, String> {
    let device_id = device_id.trim().to_string();
    if device_id.is_empty() {
        return Err("设备 ID 不能为空".to_string());
    }
    if device_id == self::device_id() {
        return Err("不能配对当前设备自身".to_string());
    }

    verify_pairing_code(&pairing_code)?;

    let peer_token = super::pairing::create_peer_token();
    let peer = PairedPeer::new(
        device_id,
        device_name.trim().to_string(),
        base_url.trim().to_string(),
        peer_token.clone(),
    );
    super::peer_store::upsert_peer(peer)?;
    Ok(peer_token)
}

/// 纯决策函数：给定已配对设备列表，判断 (device_id, token) 是否命中一条匹配。
/// 不触碰任何全局状态，便于测试注入任意 peer 列表。
pub(super) fn token_matches_peer(peers: &[PairedPeer], device_id: &str, token: &str) -> bool {
    peers
        .iter()
        .any(|peer| peer.device_id == device_id && peer.peer_token == token)
}

pub fn verify_peer_token(device_id: &str, peer_token: &str) -> bool {
    let device_id = device_id.trim();
    let peer_token = peer_token.trim();
    if device_id.is_empty() || peer_token.is_empty() {
        return false;
    }
    token_matches_peer(&super::peer_store::list_peers(), device_id, peer_token)
}

fn view_pairing_state(state: &PairingState) -> PairingCodeView {
    PairingCodeView {
        pairing_code: state.challenge.pairing_code.clone(),
        expires_at_ms: state.challenge.expires_at_ms,
        remaining_attempts: state.challenge.max_attempts.saturating_sub(state.failed_attempts),
    }
}

fn ensure_pairing_state(runtime: &mut LanRuntime) -> &PairingState {
    let should_refresh = runtime
        .pairing_state
        .as_ref()
        .map(|state| is_expired(state.challenge.expires_at_ms))
        .unwrap_or(true);
    if should_refresh {
        runtime.pairing_state = Some(PairingState {
            challenge: create_pairing_challenge(),
            failed_attempts: 0,
        });
    }
    runtime.pairing_state.as_ref().expect("pairing_state must exist")
}

fn is_expired(expires_at_ms: i64) -> bool {
    chrono::Utc::now().timestamp_millis() >= expires_at_ms
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::services::sync_transfer::lan::pairing::PairingChallenge;

    /// 串行化所有触碰全局 RUNTIME（配对状态）的测试：
    /// 注入状态与校验步骤之间不允许其它测试插入读取/替换。
    pub(crate) static PAIRING_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn inject_challenge(pairing_code: &str, expires_at_ms: i64, max_attempts: u8, failed_attempts: u8) {
        RUNTIME.lock().pairing_state = Some(PairingState {
            challenge: PairingChallenge {
                pairing_code: pairing_code.to_string(),
                expires_at_ms,
                max_attempts,
            },
            failed_attempts,
        });
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[test]
    fn current_pairing_code_creates_initial_challenge() {
        let _guard = PAIRING_TEST_LOCK.lock();
        RUNTIME.lock().pairing_state = None;
        let view = current_pairing_code();
        assert_eq!(view.pairing_code.len(), 6);
        assert!(view.pairing_code.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(view.remaining_attempts, 5);
        assert!(view.expires_at_ms > now_ms());
    }

    #[test]
    fn refresh_pairing_code_resets_failed_attempts() {
        let _guard = PAIRING_TEST_LOCK.lock();
        inject_challenge("123456", now_ms() + 60_000, 5, 4);
        let view = refresh_pairing_code();
        assert_eq!(view.remaining_attempts, 5, "刷新后尝试次数归零");
        assert_eq!(view.pairing_code.len(), 6);
        assert!(view.expires_at_ms > now_ms() + 290_000);
    }

    #[test]
    fn verify_pairing_code_wrong_code_consumes_attempt_and_locks_after_max() {
        let _guard = PAIRING_TEST_LOCK.lock();
        inject_challenge("123456", now_ms() + 60_000, 5, 0);

        for attempt in 1..=5 {
            let err = verify_pairing_code("000000").unwrap_err();
            assert_eq!(err, "配对码不正确", "第 {} 次错误应报告不正确", attempt);
        }
        assert_eq!(current_pairing_code().remaining_attempts, 0);

        let err = verify_pairing_code("000000").unwrap_err();
        assert_eq!(err, "配对码尝试次数过多，请刷新后重试");

        // 即使输入正确配对码，超过最大次数后仍被拒绝
        let err = verify_pairing_code("123456").unwrap_err();
        assert_eq!(err, "配对码尝试次数过多，请刷新后重试");
    }

    #[test]
    fn verify_pairing_code_correct_code_resets_attempts() {
        let _guard = PAIRING_TEST_LOCK.lock();
        inject_challenge("123456", now_ms() + 60_000, 5, 3);
        assert_eq!(verify_pairing_code("123456"), Ok(()));
        assert_eq!(current_pairing_code().remaining_attempts, 5, "成功后失败计数清零");
    }

    #[test]
    fn verify_pairing_code_trims_input() {
        let _guard = PAIRING_TEST_LOCK.lock();
        inject_challenge("123456", now_ms() + 60_000, 5, 0);
        assert_eq!(verify_pairing_code("  123456  "), Ok(()));
    }

    #[test]
    fn verify_pairing_code_expired_replaces_challenge_and_errors() {
        let _guard = PAIRING_TEST_LOCK.lock();
        inject_challenge("123456", now_ms() - 1, 5, 0);
        let err = verify_pairing_code("123456").unwrap_err();
        assert_eq!(err, "配对码已过期，请重新输入");
        let view = current_pairing_code();
        assert_eq!(view.remaining_attempts, 5, "过期后自动生成新挑战并清零");
        assert!(view.expires_at_ms > now_ms());
    }

    #[test]
    fn verify_pairing_code_without_state_errors_refreshed() {
        let _guard = PAIRING_TEST_LOCK.lock();
        RUNTIME.lock().pairing_state = None;
        let err = verify_pairing_code("123456").unwrap_err();
        assert_eq!(err, "配对码已刷新，请重新输入");
    }

    #[test]
    fn is_expired_boundary_is_inclusive() {
        let now = now_ms();
        assert!(is_expired(now - 1));
        assert!(is_expired(now));
        assert!(!is_expired(now + 1));
    }

    #[test]
    fn confirm_pairing_rejects_empty_device_id() {
        let err = confirm_pairing("   ".to_string(), "peer".to_string(), "http://x".to_string(), "123456".to_string())
            .unwrap_err();
        assert_eq!(err, "设备 ID 不能为空");
    }

    #[test]
    fn confirm_pairing_rejects_self_device() {
        let err = confirm_pairing(
            self::device_id(),
            "peer".to_string(),
            "http://x".to_string(),
            "123456".to_string(),
        )
        .unwrap_err();
        assert_eq!(err, "不能配对当前设备自身");
    }

    #[test]
    fn verify_peer_token_rejects_empty_or_unknown() {
        assert!(!verify_peer_token("", "tok"));
        assert!(!verify_peer_token("dev", ""));
        assert!(!verify_peer_token("", ""));
        // 未初始化 store → 无已配对设备 → 任何 token 都无效
        assert!(!verify_peer_token("dev", "tok"));
    }

    #[test]
    fn token_matches_peer_accepts_exact_device_token_pair() {
        let peers = vec![
            PairedPeer::new(
                "dev-1".to_string(),
                "one".to_string(),
                "http://1".to_string(),
                "tok-1".to_string(),
            ),
            PairedPeer::new(
                "dev-2".to_string(),
                "two".to_string(),
                "http://2".to_string(),
                "tok-2".to_string(),
            ),
        ];
        // 正向：设备 + 对应 token 命中
        assert!(token_matches_peer(&peers, "dev-1", "tok-1"));
        assert!(token_matches_peer(&peers, "dev-2", "tok-2"));
        // 反向：token 与设备必须配对，不能混用
        assert!(!token_matches_peer(&peers, "dev-1", "wrong"));
        assert!(!token_matches_peer(&peers, "dev-2", "tok-1"), "token 不能跨设备复用");
        assert!(!token_matches_peer(&peers, "dev-3", "tok-1"));
        assert!(!token_matches_peer(&peers, "", "tok-1"));
    }

    #[test]
    fn device_name_prefers_computername_then_hostname_then_default() {
        let _env_guard = crate::startup_diagnostics::tests::ENV_LOCK.lock();
        std::env::remove_var("COMPUTERNAME");
        std::env::remove_var("HOSTNAME");
        assert_eq!(device_name(), "QuickClipboard Desktop");

        std::env::set_var("HOSTNAME", "test-host");
        assert_eq!(device_name(), "test-host");

        std::env::set_var("COMPUTERNAME", "win-name");
        assert_eq!(device_name(), "win-name", "COMPUTERNAME 优先于 HOSTNAME");

        // 空白值视为未设置
        std::env::set_var("COMPUTERNAME", "   ");
        std::env::set_var("HOSTNAME", "  ");
        assert_eq!(device_name(), "QuickClipboard Desktop");

        std::env::remove_var("COMPUTERNAME");
        std::env::remove_var("HOSTNAME");
    }

    #[test]
    fn status_shape_when_nothing_running() {
        let _guard = PAIRING_TEST_LOCK.lock();
        let status = status();
        // device_id 形状：UUID v4（36 字符、版本位 '4'、全 hexdigit 或 '-'）
        assert_eq!(status.device_id.len(), 36);
        assert_eq!(&status.device_id[14..15], "4");
        assert!(status
            .device_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-'));
        assert_eq!(status.http_port, super::super::DEFAULT_HTTP_PORT, "未启动时上报默认端口");
        assert!(!status.http_running);
        assert!(!status.discovery_running);
        assert_eq!(status.pairing_code.pairing_code.len(), 6);
        assert_eq!(status.paired_count, 0, "未初始化 store 时无已配对设备");
    }
}
