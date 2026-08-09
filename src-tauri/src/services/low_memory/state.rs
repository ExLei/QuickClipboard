use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LOW_MEMORY_MODE: AtomicBool = AtomicBool::new(false);
static USER_REQUESTED_EXIT: AtomicBool = AtomicBool::new(false);
static AUTO_MANAGER_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_WINDOW_ACTIVITY_AT_MS: AtomicU64 = AtomicU64::new(0);
static EXITING_LOW_MEMORY: AtomicBool = AtomicBool::new(false);

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn is_low_memory_mode() -> bool {
    LOW_MEMORY_MODE.load(Ordering::SeqCst)
}

pub fn set_low_memory_mode(active: bool) {
    LOW_MEMORY_MODE.store(active, Ordering::SeqCst);
}

pub fn mark_window_activity() {
    LAST_WINDOW_ACTIVITY_AT_MS.store(now_unix_ms(), Ordering::SeqCst);
}

pub fn last_window_activity_at_ms() -> u64 {
    LAST_WINDOW_ACTIVITY_AT_MS.load(Ordering::SeqCst)
}

pub fn init_window_activity_timestamp() {
    mark_window_activity();
}

pub fn try_mark_auto_manager_started() -> bool {
    !AUTO_MANAGER_STARTED.swap(true, Ordering::SeqCst)
}

// 标记用户主动请求退出
pub fn set_user_requested_exit(requested: bool) {
    USER_REQUESTED_EXIT.store(requested, Ordering::SeqCst);
}

// 检查是否是用户主动请求退出
pub fn is_user_requested_exit() -> bool {
    USER_REQUESTED_EXIT.load(Ordering::SeqCst)
}

// 尝试开始退出低占用模式（防止并发退出）
pub fn try_start_exit_low_memory() -> bool {
    !EXITING_LOW_MEMORY.swap(true, Ordering::SeqCst)
}

// 完成退出低占用模式
pub fn finish_exit_low_memory() {
    EXITING_LOW_MEMORY.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::LazyLock;

    // state.rs 中的原子均为进程级全局量，触碰它们的测试必须串行。
    static STATE_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn low_memory_mode_flag_round_trips_and_defaults_to_false() {
        let _guard = STATE_TEST_LOCK.lock();
        set_low_memory_mode(false);
        assert!(!is_low_memory_mode(), "初始/关闭状态应为 false");
        set_low_memory_mode(true);
        assert!(is_low_memory_mode(), "set(true) 后应进入低占用模式");
        set_low_memory_mode(false);
        assert!(!is_low_memory_mode(), "set(false) 后应退出低占用模式");
    }

    #[test]
    fn window_activity_timestamp_is_set_to_now_and_monotonic() {
        let _guard = STATE_TEST_LOCK.lock();
        let before = now_unix_ms();
        mark_window_activity();
        let stamped = last_window_activity_at_ms();
        let after = now_unix_ms();
        assert!(stamped > 0, "时间戳必须非零");
        assert!(
            before <= stamped && stamped <= after,
            "时间戳应落在调用时刻区间内: before={before}, stamped={stamped}, after={after}"
        );
        let first = stamped;
        std::thread::sleep(std::time::Duration::from_millis(5));
        mark_window_activity();
        let second = last_window_activity_at_ms();
        assert!(second >= first, "后续标记不应使时间戳回退");
    }

    #[test]
    fn init_window_activity_timestamp_sets_nonzero_timestamp() {
        let _guard = STATE_TEST_LOCK.lock();
        init_window_activity_timestamp();
        assert!(last_window_activity_at_ms() > 0, "初始化后时间戳应非零");
    }

    #[test]
    fn user_requested_exit_flag_round_trips_and_defaults_to_false() {
        let _guard = STATE_TEST_LOCK.lock();
        set_user_requested_exit(false);
        assert!(!is_user_requested_exit(), "初始状态应为 false");
        set_user_requested_exit(true);
        assert!(is_user_requested_exit(), "set(true) 后应标记为用户主动退出");
        set_user_requested_exit(false);
        assert!(!is_user_requested_exit(), "set(false) 后应清除标记");
    }

    #[test]
    fn auto_manager_started_flag_is_set_exactly_once() {
        let _guard = STATE_TEST_LOCK.lock();
        assert!(try_mark_auto_manager_started(), "第一次调用应成功抢占");
        assert!(!try_mark_auto_manager_started(), "第二次调用应失败");
        assert!(!try_mark_auto_manager_started(), "第三次调用仍应失败");
    }

    #[test]
    fn exit_low_memory_guard_blocks_concurrent_exit_until_finished() {
        let _guard = STATE_TEST_LOCK.lock();
        finish_exit_low_memory();
        assert!(try_start_exit_low_memory(), "空闲时应能成功开始退出");
        assert!(!try_start_exit_low_memory(), "退出进行中应拒绝并发退出");
        assert!(!try_start_exit_low_memory(), "退出进行中继续拒绝");
        finish_exit_low_memory();
        assert!(
            try_start_exit_low_memory(),
            "finish 后应恢复为可再次退出"
        );
        finish_exit_low_memory();
    }
}
