#[cfg(not(target_os = "windows"))]
use enigo::{Enigo, Direction, Key, Keyboard, Settings};

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_INSERT, VK_MENU,
    VK_CONTROL, VK_LWIN, VK_RWIN, VK_SHIFT, VK_V,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::GetCurrentProcessId;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};

#[cfg(target_os = "windows")]
fn is_key_pressed(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

// 注入单个按键事件，返回是否成功；SendInput 返回 0 表示事件被系统阻塞
// （如向高完整性前台窗口注入时被 UIPI 拦截；返回值不区分具体原因）
#[cfg(target_os = "windows")]
fn send_key(vk: u16, up: bool) -> bool {
    send_key_ex(vk, up, false)
}

#[cfg(target_os = "windows")]
fn send_key_ex(vk: u16, up: bool, extended: bool) -> bool {
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: crate::services::system::raw_input::PASTE_INPUT_MARKER,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 1 }
}

#[cfg(target_os = "windows")]
use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[cfg(target_os = "windows")]
static CURRENT_TRIGGER_KEY: Mutex<Option<u16>> = Mutex::new(None);

#[cfg(target_os = "windows")]
static PASTE_SIMULATION_LOCK: Mutex<()> = Mutex::new(());

// 释放注入被前台高完整性窗口拦截而卡住的粘贴主键（V/Insert）：主键 down
// 已送达、up 被拦时逻辑卡住。raw_input 只跟踪修饰键物理态，主键无法像
// 修饰键那样比对物理状态，但序列结束后主键本就该处于抬起状态，直接由
// 前台切换回调重试释放。单槽即可：先后卡住不同主键（V 与 Insert 分属两
// 种粘贴模式，切换必经设置页、必引发前台切换自愈）实际不可达，槽位覆盖
// 被自愈先行阻断
#[cfg(target_os = "windows")]
static STUCK_PASTE_KEY: Mutex<Option<u16>> = Mutex::new(None);

// 等待挂起前台切换完成的上限：前台切换通常在数十毫秒内完成，300ms 兜底
#[cfg(target_os = "windows")]
const FOREGROUND_SWITCH_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(300);

// 「刚隐藏」判定窗口：埋点与随后的注入之间除百毫秒级 sleep 外，还有
// 数据库读取与剪贴板写入等 IO（大条目、慢盘可超过 1 秒），取 3 秒覆盖
// 慢路径。过期只导致「该等而没等」（退回卡键检测兜底），不会引入误
// 等待——等待条件仍要求前台尚未离开被隐藏窗口（见 wait_foreground_switch）
#[cfg(target_os = "windows")]
const OWN_WINDOW_HIDE_RECENT: std::time::Duration = std::time::Duration::from_secs(3);

// 本进程前台窗口（主窗口、低内存面板）最近一次「确有前台切换待完成」的
// 隐藏记录：wait_away 为需等待离开的前台窗口句柄（0 表示前台为空、等待
// 其落定）。隐藏若发生在该窗口仍处于前台时，Windows 会异步切换前台，随
// 后的粘贴序列需等待切换完成再注入，避免序列跨过切换时刻造成「前段注入
// 成功、后段被 UIPI 拦截」的修饰键卡住（issue #496）。仅在被隐藏窗口
// 恰持前台时才记录（见 note_own_window_hidden），无待完成切换的隐藏
// 不入账。时刻用 Instant 单调钟而非 SystemTime 墙钟，避免系统校时
// 回拨导致误判
#[cfg(target_os = "windows")]
static LAST_OWN_WINDOW_HIDE: Mutex<Option<HideNote>> = Mutex::new(None);

// LAST_OWN_WINDOW_HIDE 的条目：隐藏时刻 + 已确认待离开的前台窗口句柄
#[cfg(target_os = "windows")]
struct HideNote {
    hidden_at: std::time::Instant,
    wait_away: isize,
}

// raw_input 物理修饰键状态不可用的提示只记录一次，避免在前台切换回调里刷屏
#[cfg(target_os = "windows")]
static PHYSICAL_STATE_MISSING_LOGGED: AtomicBool = AtomicBool::new(false);

// 最近一次释放注入仍被拦截的卡住修饰键集合（位掩码，bit 下标与
// entries() 遍历顺序一致，仅在 release_stuck_modifiers 内读写）：用于
// 卡键日志去重，卡键未愈期间前台切换回调的重试不再逐次刷屏；释放成功
// 即清除，同键再次卡住按新状态重新记录
#[cfg(target_os = "windows")]
static STUCK_MODIFIERS_LOGGED: AtomicU8 = AtomicU8::new(0);

#[cfg(target_os = "windows")]
pub fn set_trigger_key_from_shortcut(shortcut: &str) {
    if let Some(vk) = parse_shortcut_key_vk(shortcut) {
        *CURRENT_TRIGGER_KEY.lock().unwrap_or_else(|error| error.into_inner()) = Some(vk);
    }
}

#[cfg(target_os = "windows")]
pub fn set_trigger_key_raw(vk: u16) {
    *CURRENT_TRIGGER_KEY.lock().unwrap_or_else(|error| error.into_inner()) = Some(vk);
}

#[cfg(target_os = "windows")]
fn take_trigger_key() -> Option<u16> {
    CURRENT_TRIGGER_KEY
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

// 从快捷键字符串解析非修饰键虚拟键码
#[cfg(target_os = "windows")]
fn parse_shortcut_key_vk(shortcut: &str) -> Option<u16> {
    let key = shortcut
        .split('+')
        .last()?
        .trim();
    if key.is_empty() {
        return None;
    }
    if key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_uppercase() {
            return Some(ch as u16);
        }
        if ch.is_ascii_digit() {
            return Some(ch as u16);
        }
        return None;
    }
    match key.to_uppercase().as_str() {
        "INSERT" => Some(0x2D),
        other => {
            if let Some(num) = other.strip_prefix("F").and_then(|n| n.parse::<u16>().ok()) {
                if (1..=24).contains(&num) {
                    return Some(0x6F + num);
                }
            }
            None
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct ModifierState {
    ctrl: bool,
    shift: bool,
    alt: bool,
    lwin: bool,
    rwin: bool,
}

#[cfg(target_os = "windows")]
impl ModifierState {
    fn record() -> Self {
        Self {
            ctrl: is_key_pressed(VK_CONTROL.0),
            shift: is_key_pressed(VK_SHIFT.0),
            alt: is_key_pressed(VK_MENU.0),
            lwin: is_key_pressed(VK_LWIN.0),
            rwin: is_key_pressed(VK_RWIN.0),
        }
    }

    // (vk, 该键在本状态中是否按下)，顺序与 raw_input 物理态元组一致：
    // Ctrl / Shift / Alt / LWin / RWin。release_stuck_modifiers 依赖逻辑态
    // 与物理态按同一顺序对齐，禁止单独调整任一侧的顺序
    fn entries(&self) -> [(u16, bool); 5] {
        [
            (VK_CONTROL.0, self.ctrl),
            (VK_SHIFT.0, self.shift),
            (VK_MENU.0, self.alt),
            (VK_LWIN.0, self.lwin),
            (VK_RWIN.0, self.rwin),
        ]
    }

    // 释放被卡住的修饰键（逻辑按下、物理已释放——注入的 up 被 UIPI 拦截所致，
    // 会导致回车/方向键全局失效、快捷键失配，issue #496）。
    // 读序说明：先读物理（raw_input 原子量）再读逻辑（GetAsyncKeyState）。
    // 逻辑读取同步反映物理按键，物理原子量经 raw_input 线程更新存在毫秒级滞后，
    // 两次读取之间用户恰好按下物理键时存在理论上的误判窗口；所有调用点
    // （粘贴序列结束、前台切换回调）与用户按键时刻基本错开，风险可忽略。
    // 已知边界：若 raw_input 线程长时间阻塞导致原子量持久过期，此处会把
    // 用户正按着的修饰键误判为卡住并抬起；根治需为物理态增加新鲜度校验，
    // 留待后续
    fn release_stuck_modifiers() {
        let Some(physical) = Self::physical() else {
            // raw_input 未启动或初始化失败时物理态不可用，卡键检测与自愈
            // 整体失效；只提示一次，便于诊断
            if !PHYSICAL_STATE_MISSING_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!("[paste] raw_input 物理修饰键状态不可用，卡键检测与自愈失效");
            }
            return;
        };
        let logical = Self::record();
        // 日志去重（对齐 PHYSICAL_STATE_MISSING_LOGGED 的设计意图）：卡键
        // 未愈期间前台切换回调每次都会进入此函数，逐次打印「检测到」与
        // 「被拦截」构成刷屏。仅当某键的卡住状态是新出现时记录检测与拦截
        // 日志、重试期间静默，释放成功（且此前记录过卡住）时补一条成功
        // 日志、去重位随即清除——与主键自愈（retry_stuck_paste_key）的
        // 成功日志对称
        let prev_logged = STUCK_MODIFIERS_LOGGED.load(Ordering::Relaxed);
        let mut logged = 0u8;
        for (index, ((vk, logical_down), (_, physical_down))) in logical
            .entries()
            .into_iter()
            .zip(physical.entries())
            .enumerate()
        {
            if logical_down && !physical_down {
                if prev_logged & (1 << index) == 0 {
                    eprintln!("[paste] 检测到卡住的修饰键 vk=0x{:X}，尝试释放", vk);
                }
                if !send_key(vk, true) {
                    // 释放注入仍被拦截（前台还是高完整性窗口）：由前台切换
                    // 事件回调（focus.rs）在前台切回可注入窗口后重试释放。
                    // 仍卡住才置位去重：释放成功即清除，同键再次卡住按
                    // 新状态重新记录
                    logged |= 1 << index;
                    if prev_logged & (1 << index) == 0 {
                        eprintln!(
                            "[paste] 卡住的修饰键 vk=0x{:X} 释放注入被拦截，等待前台切换后重试",
                            vk
                        );
                    }
                } else if prev_logged & (1 << index) != 0 {
                    eprintln!("[paste] 卡住的修饰键 vk=0x{:X} 已在前台切换后释放", vk);
                }
            }
        }
        STUCK_MODIFIERS_LOGGED.store(logged, Ordering::Relaxed);
    }

    // 释放与粘贴快捷键冲突的修饰键（Ctrl/Shift/Win）。Alt 不在此处释放：
    // 调用方需先释放触发键再释放 Alt，顺序有语义，勿并入。基于 entries()
    // 遍历，与 apply / release_stuck_modifiers 共享同一键序
    fn release_conflicting(&self) {
        let mut released = false;
        for (vk, pressed) in self.entries() {
            if pressed && vk != VK_MENU.0 {
                send_key(vk, true);
                released = true;
            }
        }
        if released {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn physical() -> Option<Self> {
        crate::services::system::raw_input::get_physical_modifier_keys_state().map(
            |(ctrl, shift, alt, lwin, rwin)| Self {
                ctrl,
                shift,
                alt,
                lwin,
                rwin,
            },
        )
    }

    fn apply(&self) {
        for (vk, pressed) in self.entries() {
            Self::set_key_state(vk, pressed);
        }
    }

    fn set_key_state(vk: u16, pressed: bool) {
        if is_key_pressed(vk) != pressed {
            send_key(vk, !pressed);
        }
    }

    fn restore_current_physical(&self) {
        for pass in 0..2 {
            Self::physical().unwrap_or(*self).apply();
            if pass == 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        // 序列结束后校验并解除卡键（issue #496）：立即尝试一次；若释放
        // 注入仍被前台高完整性窗口持续拦截（UIPI），由前台切换事件回调
        // （focus.rs 的 try_release_stuck_keys）在前台切到可注入
        // 窗口后自动重试释放，无需轮询、无时间上限
        Self::release_stuck_modifiers();
    }
}

// 前台切换事件回调：此刻前台已切换到新窗口，卡键释放注入不再被旧前台
// 拦截，重试一次；粘贴序列进行中（锁被持有）时让位，不打扰其修饰键
// 记录与恢复
#[cfg(target_os = "windows")]
pub(crate) fn try_release_stuck_keys() {
    if let Ok(_guard) = PASTE_SIMULATION_LOCK.try_lock() {
        ModifierState::release_stuck_modifiers();
        retry_stuck_paste_key();
    }
}

// 重试释放卡住的粘贴主键：主键无物理态可比对（raw_input 只跟踪修饰键），
// 以「逻辑仍按下且所属序列已结束」判定卡住。已知边界：用户恰在此刻物理
// 按着该主键时会误抬一次，概率与修饰键自愈的理论误判窗口同级，可忽略。
// 释放仍被拦截则保留记录，等待下一次前台切换重试
#[cfg(target_os = "windows")]
fn retry_stuck_paste_key() {
    let mut stuck = STUCK_PASTE_KEY
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(vk) = *stuck else {
        return;
    };
    if !is_key_pressed(vk) {
        // 逻辑已不在按下状态（用户物理按过该键、或状态已自行恢复）
        *stuck = None;
        return;
    }
    if send_key(vk, true) {
        eprintln!("[paste] 卡住的粘贴主键 vk=0x{:X} 已在前台切换后释放", vk);
        *stuck = None;
    }
}

// 前台窗口是否属于本进程；查询失败（窗口销毁中）也按本进程处理——
// 销毁本身就会引发前台切换，等待其完成更安全
#[cfg(target_os = "windows")]
fn foreground_belongs_to_own_process(foreground: windows::Win32::Foundation::HWND) -> bool {
    let mut process_id: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(foreground, Some(&mut process_id)) == 0
            || process_id == GetCurrentProcessId()
    }
}

// 等待挂起的前台切换完成（issue #496）：隐藏仍处于前台的窗口后，Windows
// 异步切换前台；若注入序列跨过切换时刻，会出现「前段注入成功、后段被
// UIPI 拦截」的不对称。wait_away 为需等待离开的窗口句柄（0 表示前台为
// 空、等待其落定即可）。超时后放行——此时注入可能落在本进程窗口上导致
// 粘贴未生效，但整段序列同生共死，不会卡住修饰键；序列中段前台仍可能
// 被第三方切换（抢焦点弹窗等），残余情况由卡键检测兜底
#[cfg(target_os = "windows")]
fn wait_foreground_switch(wait_away: isize) {
    // 不以「窗口已重新可见」短路放行：隐藏经 Tauri 主线程异步派发，埋点
    // 记录时窗口可能尚未真正隐藏，可见性无法区分「已重新显示」（无待完
    // 成切换）与「隐藏未落地」（恰恰需要等待）——后者误放行会让注入序列
    // 跨过随后的隐藏落地与前台切换，重现「前段注入成功、后段被 UIPI 拦
    // 截」的不对称。重新显示场景改由显示路径清除埋点处理
    // （clear_own_window_hidden_pending），此处只认定一个事实：前台离开
    // 被隐藏窗口即切换完成
    let deadline = std::time::Instant::now() + FOREGROUND_SWITCH_TIMEOUT;
    while std::time::Instant::now() < deadline {
        unsafe {
            let foreground = GetForegroundWindow();
            let foreground_val = foreground.0 as isize;
            // 前台已离开被隐藏窗口即切换完成。若前台仍等于该句柄值但已
            // 不属于本进程，说明被隐藏窗口销毁后（如进入低占用模式时
            // destroy_all_webviews）句柄值被其他进程的新窗口复用且恰为
            // 前台——原窗口引发的前台切换早已完成，等待无意义，同样放行
            if foreground_val != 0
                && (foreground_val != wait_away
                    || !foreground_belongs_to_own_process(foreground))
            {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

// 记录本进程前台窗口（主窗口、低内存面板）刚隐藏：随后的
// 粘贴序列需等待前台切换完成再注入，避免序列跨越切换时刻造成修饰键卡住
// （issue #496）。hidden_hwnd 为被隐藏窗口的句柄（未知时传 None）：仅当
// 该窗口此刻仍持前台、确有切换待完成时才记录等待；其余情况（前台已在
// 别处——如粘贴目标为本进程其他存活窗口、或句柄未知）一律不记录——
// 既不覆盖可能仍挂起的既有埋点，也不以「前台属于本进程」猜测被隐藏
// 窗口：receive-box 等本进程存活窗口不在隐藏之列，猜测会把它们误当作
// 被隐藏窗口、导致粘贴白等满 300ms
#[cfg(target_os = "windows")]
pub(crate) fn note_own_window_hidden(hidden_hwnd: Option<isize>) {
    let wait_away = unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            // 前台为空：正处于某次切换的中间态。埋点可能在隐藏落地后
            // 调用（面板 UiEvent::Hidden 回调），此刻的中间态可能正是
            // 本次隐藏引发的切换，保守等待其落定再注入；埋点必先于
            // 隐藏/销毁落地的调用方（destroy_all_webviews）以各自守卫
            // 跳过，不会进入此分支
            0
        } else {
            match hidden_hwnd {
                Some(hidden) if hidden == foreground.0 as isize => hidden,
                _ => return,
            }
        }
    };
    *LAST_OWN_WINDOW_HIDE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(HideNote {
        hidden_at: std::time::Instant::now(),
        wait_away,
    });
}

// 窗口重新显示（主窗口 show_main_window、低内存面板）：此前隐藏留下的
// 「待完成前台切换」不再成立——窗口将重获前台或切换已被打断，清除埋点，
// 随后的粘贴序列不再等待（注入落在本进程窗口上是明示接受的设计场景）。
// 与以可见性短路放行相比，显示路径的清除是确定性信号：隐藏未落地时窗口
// 不会走显示路径，埋点必然保留、等待照常进行
#[cfg(target_os = "windows")]
pub(crate) fn clear_own_window_hidden_pending() {
    *LAST_OWN_WINDOW_HIDE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

// 若最近刚隐藏过本进程前台窗口且该隐藏留下了待完成的前台切换，
// 返回需等待离开的窗口句柄；否则返回 None（无需等待）
#[cfg(target_os = "windows")]
fn pending_foreground_switch() -> Option<isize> {
    let last = LAST_OWN_WINDOW_HIDE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match last.as_ref() {
        Some(note) if note.hidden_at.elapsed() < OWN_WINDOW_HIDE_RECENT => {
            Some(note.wait_away)
        }
        _ => None,
    }
}

// 模拟粘贴
#[cfg(target_os = "windows")]
pub fn simulate_paste() -> Result<(), String> {
    // 完整序列必须串行，避免连续粘贴互相误判对方注入的修饰键。
    let _paste_guard = PASTE_SIMULATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let settings = crate::get_settings();

    // 本进程窗口（主窗口、低内存面板）隐藏引发的前台切换
    // 是异步的，等待切换完成让注入序列完整落在同一个前台窗口上，避免
    // 「前段注入成功、后段被 UIPI 拦截」的不对称造成修饰键卡住（issue
    // #496）。仅在该隐藏留下了待完成的前台切换时等待；热键直粘、pin
    // 模式、粘贴目标为本进程窗口（如文本编辑器）等场景不等待、不增加延迟
    if let Some(wait_away) = pending_foreground_switch() {
        wait_foreground_switch(wait_away);
    }

    if settings.paste_shortcut_mode == "ctrl_v" {
        simulate_paste_ctrl_v()
    } else {
        simulate_paste_shift_insert()
    }
}

// 粘贴序列：记录修饰键 → 释放冲突键 → 纯净注入 → 对齐物理状态。
// 修饰键按下被拦截（UIPI）时放弃本次注入：整段序列同生共死，继续注入
// 没有意义，中止并记录日志便于诊断；修饰键释放侧被拦截由序列末的卡键
// 检测与前台切换回调自愈，主键释放侧被拦截由 STUCK_PASTE_KEY 记录、
// 前台切换回调重试释放（issue #496）
#[cfg(target_os = "windows")]
fn run_paste_sequence(modifier_vk: u16, key_vk: u16, key_extended: bool) {
    let mods = ModifierState::record();
    mods.release_conflicting();
    if let Some(vk) = take_trigger_key() {
        // 触发键是用户物理按下的键（setter 只在热键触发路径调用），其 up
        // 注入即使被 UIPI 拦截，用户松手时的物理 up 也会清除逻辑状态，
        // 不存在永久卡键，无需检测与自愈
        send_key(vk, true);
    }
    if mods.alt {
        send_key(VK_MENU.0, true);
    }

    if send_key(modifier_vk, false) {
        if send_key_ex(key_vk, false, key_extended) {
            std::thread::sleep(std::time::Duration::from_millis(8));
            if !send_key_ex(key_vk, true, key_extended) {
                // 主键释放被拦截（如序列中段前台被切到高完整性窗口）：down 已
                // 送达、up 被拦，主键逻辑态卡住。无修饰键类的全局副作用，也
                // 不会持续 autorepeat——自动重复由键盘硬件对物理按键生成，
                // 注入的单次 down 不触发；实际影响是污染 GetAsyncKeyState
                // 查询方、该键后续物理按下可能被应用计为 repeat。记录后由
                // 前台切换回调重试释放（issue #496）
                *STUCK_PASTE_KEY
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(key_vk);
                eprintln!(
                    "[paste] 主键 vk=0x{:X} 释放注入被拦截，等待前台切换后重试释放",
                    key_vk
                );
            }
        } else {
            // 主键按下被拦截：主键未进入按下状态，再注入释放是对未按下键的
            // 幽灵事件，且会把从未按下的键误记入卡键自愈槽位、留下「down 已
            // 送达」的失实日志，污染最关键失败场景的诊断——跳过释放注入。
            // 修饰键释放仍照常进行（其被拦由卡键检测兜底），记录日志便于诊断
            eprintln!("[paste] 主键 vk=0x{:X} 按下注入被拦截，跳过释放注入", key_vk);
        }
        if !send_key(modifier_vk, true) {
            // 释放注入被拦截（如序列中段前台被切到高完整性窗口）：修饰键
            // 卡在按下状态，由序列末的卡键检测立即尝试释放、前台切换
            // 回调兜底重试（issue #496）
            eprintln!(
                "[paste] 修饰键 vk=0x{:X} 释放注入被拦截，等待卡键检测自愈",
                modifier_vk
            );
        }
    } else {
        // 按下注入被拦截（UIPI 等）：中止后续序列，粘贴未发生，记录日志
        // 便于诊断（issue #496）
        eprintln!(
            "[paste] 修饰键 vk=0x{:X} 按下注入被拦截，中止本次粘贴序列",
            modifier_vk
        );
    }

    mods.restore_current_physical();
}

// Shift+Insert 粘贴
#[cfg(target_os = "windows")]
fn simulate_paste_shift_insert() -> Result<(), String> {
    run_paste_sequence(VK_SHIFT.0, VK_INSERT.0, true);
    Ok(())
}

// Ctrl+V 粘贴
#[cfg(target_os = "windows")]
fn simulate_paste_ctrl_v() -> Result<(), String> {
    run_paste_sequence(VK_CONTROL.0, VK_V.0, false);
    Ok(())
}

// 模拟粘贴
#[cfg(not(target_os = "windows"))]
pub fn simulate_paste() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("创建键盘模拟器失败: {}", e))?;

    enigo.key(Key::Control, Direction::Press)
        .map_err(|e| format!("按下Ctrl失败: {}", e))?;
    
    enigo.key(Key::Unicode('v'), Direction::Press)
        .map_err(|e| format!("按下V失败: {}", e))?;
    
    std::thread::sleep(std::time::Duration::from_millis(8));
    
    enigo.key(Key::Unicode('v'), Direction::Release)
        .map_err(|e| format!("释放V失败: {}", e))?;
    
    enigo.key(Key::Control, Direction::Release)
        .map_err(|e| format!("释放Ctrl失败: {}", e))?;
    
    Ok(())
}

