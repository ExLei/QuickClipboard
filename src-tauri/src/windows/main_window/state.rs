use parking_lot::RwLock;
use once_cell::sync::Lazy;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowState {
    Hidden,
    Visible,
    Minimized,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapEdge {
    None,
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone)]
pub struct MainWindowState {
    pub state: WindowState,
    pub is_dragging: bool,
    pub is_snapped: bool,
    pub is_hidden: bool,
    pub is_pinned: bool,
    pub snap_edge: SnapEdge,
    pub snap_position: Option<(i32, i32)>,
    pub snap_monitor_id: Option<String>,
    pub snap_ratio: Option<f64>,
    pub clipboard_refresh_pending: bool,
    pub favorites_refresh_pending: bool,
    pub groups_refresh_pending: bool,
}

impl Default for MainWindowState {
    fn default() -> Self {
        Self {
            state: WindowState::Hidden,
            is_dragging: false,
            is_snapped: false,
            is_hidden: false,
            is_pinned: false,
            snap_edge: SnapEdge::None,
            snap_position: None,
            snap_monitor_id: None,
            snap_ratio: None,
            clipboard_refresh_pending: false,
            favorites_refresh_pending: false,
            groups_refresh_pending: false,
        }
    }
}

static WINDOW_STATE: Lazy<RwLock<MainWindowState>> = 
    Lazy::new(|| RwLock::new(MainWindowState::default()));

pub fn get_window_state() -> MainWindowState {
    WINDOW_STATE.read().clone()
}

pub fn set_window_state(state: WindowState) {
    WINDOW_STATE.write().state = state;
}

pub fn is_main_window_visible_for_updates() -> bool {
    let state = WINDOW_STATE.read();
    state.state == WindowState::Visible && !state.is_hidden
}

pub fn set_dragging(is_dragging: bool) {
    WINDOW_STATE.write().is_dragging = is_dragging;
}

pub fn set_snap_edge(
    edge: SnapEdge,
    position: Option<(i32, i32)>,
    monitor_id: Option<String>,
    ratio: Option<f64>,
) {
    let mut state = WINDOW_STATE.write();
    state.is_snapped = edge != SnapEdge::None;
    state.snap_edge = edge;
    state.snap_position = position;
    state.snap_monitor_id = monitor_id;
    state.snap_ratio = ratio;
}

pub fn set_hidden(is_hidden: bool) {
    WINDOW_STATE.write().is_hidden = is_hidden;
}

pub fn mark_clipboard_refresh_pending() {
    WINDOW_STATE.write().clipboard_refresh_pending = true;
}

pub fn mark_favorites_refresh_pending() {
    let mut state = WINDOW_STATE.write();
    state.favorites_refresh_pending = true;
    state.clipboard_refresh_pending = true;
}

pub fn mark_groups_refresh_pending() {
    WINDOW_STATE.write().groups_refresh_pending = true;
}

pub fn take_pending_refresh_flags() -> (bool, bool, bool) {
    let mut state = WINDOW_STATE.write();
    let flags = (
        state.clipboard_refresh_pending,
        state.favorites_refresh_pending,
        state.groups_refresh_pending,
    );
    state.clipboard_refresh_pending = false;
    state.favorites_refresh_pending = false;
    state.groups_refresh_pending = false;
    flags
}

pub fn is_snapped() -> bool {
    WINDOW_STATE.read().is_snapped
}

pub fn clear_snap() {
    let mut state = WINDOW_STATE.write();
    state.is_snapped = false;
    state.is_hidden = false;
    state.snap_edge = SnapEdge::None;
    state.snap_position = None;
    state.snap_monitor_id = None;
    state.snap_ratio = None;
}

pub fn set_pinned(is_pinned: bool) {
    WINDOW_STATE.write().is_pinned = is_pinned;
}

pub fn is_pinned() -> bool {
    WINDOW_STATE.read().is_pinned
}

#[cfg(test)]
mod tests {
    use super::*;

    // 本模块的全局 WINDOW_STATE 是进程级单例，cargo test 多线程并行执行，
    // 用本地互斥锁串行化本文件内的测试，避免互相污染。
    static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn reset_state() {
        clear_snap();
        set_hidden(false);
        set_window_state(WindowState::Hidden);
        set_pinned(false);
        set_dragging(false);
        take_pending_refresh_flags();
    }

    #[test]
    fn default_state_is_hidden_and_clean() {
        // 直接断言生产类型 MainWindowState::default()，不经过 reset_state() 辅助函数
        let state = MainWindowState::default();
        assert_eq!(state.state, WindowState::Hidden);
        assert!(!state.is_dragging);
        assert!(!state.is_snapped);
        assert!(!state.is_hidden);
        assert!(!state.is_pinned);
        assert_eq!(state.snap_edge, SnapEdge::None);
        assert_eq!(state.snap_position, None);
        assert_eq!(state.snap_monitor_id, None);
        assert_eq!(state.snap_ratio, None);
        assert!(!state.clipboard_refresh_pending);
        assert!(!state.favorites_refresh_pending);
        assert!(!state.groups_refresh_pending);
    }

    #[test]
    fn window_state_round_trip_and_visibility_rule() {
        let _lock = TEST_LOCK.lock();
        reset_state();
        set_window_state(WindowState::Visible);
        assert_eq!(get_window_state().state, WindowState::Visible);
        assert!(is_main_window_visible_for_updates(), "Visible 且未隐藏应视为可见");

        set_window_state(WindowState::Minimized);
        assert!(!is_main_window_visible_for_updates(), "Minimized 不算可见");

        set_window_state(WindowState::Visible);
        set_hidden(true);
        assert!(
            !is_main_window_visible_for_updates(),
            "Visible 但隐藏也不算可见"
        );
        set_hidden(false);
        assert!(is_main_window_visible_for_updates());
    }

    #[test]
    fn set_snap_edge_marks_snapped_only_for_real_edge() {
        let _lock = TEST_LOCK.lock();
        reset_state();
        set_snap_edge(SnapEdge::Left, Some((10, 20)), Some("monitor-1".into()), Some(0.25));
        assert!(is_snapped());
        let state = get_window_state();
        assert_eq!(state.snap_edge, SnapEdge::Left);
        assert_eq!(state.snap_position, Some((10, 20)));
        assert_eq!(state.snap_monitor_id.as_deref(), Some("monitor-1"));
        assert_eq!(state.snap_ratio, Some(0.25));

        set_snap_edge(SnapEdge::None, None, None, None);
        assert!(!is_snapped(), "None 边不应标记为 snapped");
        let state = get_window_state();
        assert_eq!(state.snap_edge, SnapEdge::None);
        assert_eq!(state.snap_position, None);
        assert_eq!(state.snap_monitor_id, None);
        assert_eq!(state.snap_ratio, None);
    }

    #[test]
    fn pending_refresh_flags_follow_marking_rules() {
        let _lock = TEST_LOCK.lock();
        reset_state();
        assert_eq!(take_pending_refresh_flags(), (false, false, false));

        mark_clipboard_refresh_pending();
        assert_eq!(
            take_pending_refresh_flags(),
            (true, false, false),
            "clipboard 只标记自身"
        );
        assert_eq!(take_pending_refresh_flags(), (false, false, false), "take 后复位");

        mark_favorites_refresh_pending();
        assert_eq!(
            take_pending_refresh_flags(),
            (true, true, false),
            "favorites 连带标记 clipboard"
        );

        mark_groups_refresh_pending();
        assert_eq!(
            take_pending_refresh_flags(),
            (false, false, true),
            "groups 只标记自身"
        );

        mark_clipboard_refresh_pending();
        mark_favorites_refresh_pending();
        mark_groups_refresh_pending();
        assert_eq!(take_pending_refresh_flags(), (true, true, true));
    }

    #[test]
    fn clear_snap_resets_snap_and_hidden_together() {
        let _lock = TEST_LOCK.lock();
        reset_state();
        set_snap_edge(SnapEdge::Right, Some((5, 5)), Some("m".into()), Some(1.0));
        set_hidden(true);
        clear_snap();
        assert!(!is_snapped());
        let state = get_window_state();
        assert!(!state.is_hidden, "clear_snap 必须同时清除 hidden");
        assert_eq!(state.snap_edge, SnapEdge::None);
        assert_eq!(state.snap_position, None);
        assert_eq!(state.snap_monitor_id, None);
        assert_eq!(state.snap_ratio, None);
    }

    #[test]
    fn pin_and_dragging_flags_round_trip() {
        let _lock = TEST_LOCK.lock();
        reset_state();
        assert!(!is_pinned());
        set_pinned(true);
        assert!(is_pinned());
        assert!(get_window_state().is_pinned);
        set_pinned(false);
        assert!(!is_pinned());

        set_dragging(true);
        assert!(get_window_state().is_dragging);
        set_dragging(false);
        assert!(!get_window_state().is_dragging);
    }
}

