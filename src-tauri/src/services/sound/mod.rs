use once_cell::sync::Lazy;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static BUILTIN_COPY_SOUND: &[u8] = include_bytes!("../../../../sounds/copy.mp3");
static BUILTIN_PASTE_SOUND: &[u8] = include_bytes!("../../../../sounds/paste.mp3");
static BUILTIN_SCROLL_SOUND: &[u8] = include_bytes!("../../../../sounds/roll.mp3");
const AUDIO_WARMUP_DURATION: Duration = Duration::from_millis(200);
const AUDIO_IDLE_RELEASE_DELAY: Duration = Duration::from_secs(1);
const AUDIO_POLL_INTERVAL: Duration = Duration::from_millis(50);

// 记录最后一次粘贴音效播放的时间戳
static LAST_PASTE_SOUND_TIME_MS: AtomicU64 = AtomicU64::new(0);

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis() as u64
}

pub fn mark_paste_operation() {
    LAST_PASTE_SOUND_TIME_MS.store(current_time_ms(), Ordering::Relaxed);
}

// 距离上次粘贴音效播放不足 300ms 时抑制“复制-立即”音效
fn is_immediate_copy_sound_suppressed(now_ms: u64) -> bool {
    let last_paste_time = LAST_PASTE_SOUND_TIME_MS.load(Ordering::Relaxed);
    last_paste_time > 0 && now_ms.saturating_sub(last_paste_time) < 300
}

enum SoundCommand {
    PlayFile(PathBuf, f32),
    PlayBytes(&'static [u8], f32),
    PlayBeep(f32, u64, f32),
}

static SOUND_SENDER: Lazy<Sender<SoundCommand>> = Lazy::new(|| {
    let (tx, rx) = mpsc::channel::<SoundCommand>();

    if let Err(error) = thread::Builder::new()
        .name("audio-player".into())
        .spawn(move || audio_thread_loop(rx))
    {
        eprintln!("创建音效播放线程失败: {}", error);
    }

    tx
});

fn get_default_device_name() -> Option<String> {
    rodio::cpal::default_host()
        .default_output_device()
        .and_then(|device| device.name().ok())
}

struct AudioContext {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    device_name: Option<String>,
    sinks: Vec<Sink>,
}

impl AudioContext {
    fn try_new() -> Option<Self> {
        let (stream, handle) = OutputStream::try_default().ok()?;
        let device_name = get_default_device_name();
        Some(Self {
            _stream: stream,
            handle,
            device_name,
            sinks: Vec::new(),
        })
    }

    fn device_changed(&self) -> bool {
        get_default_device_name() != self.device_name
    }

    fn play(&mut self, cmd: &SoundCommand) -> Result<(), String> {
        let sink = match cmd {
            SoundCommand::PlayFile(path, volume) => play_file(&self.handle, path, *volume),
            SoundCommand::PlayBytes(bytes, volume) => play_bytes(&self.handle, bytes, *volume),
            SoundCommand::PlayBeep(freq, dur, vol) => play_beep(&self.handle, *freq, *dur, *vol),
        }?;

        self.sinks.push(sink);
        Ok(())
    }

    fn remove_finished_sinks(&mut self) {
        self.sinks.retain(|sink| !sink.empty());
    }

    fn is_idle(&self) -> bool {
        self.sinks.is_empty()
    }
}

fn create_audio_context() -> Option<AudioContext> {
    let context = AudioContext::try_new();
    if context.is_some() {
        thread::sleep(AUDIO_WARMUP_DURATION);
    }
    context
}

fn audio_thread_loop(rx: mpsc::Receiver<SoundCommand>) {
    let mut ctx: Option<AudioContext> = None;
    let mut idle_since: Option<Instant> = None;

    loop {
        let timeout = if ctx.is_some() {
            AUDIO_POLL_INTERVAL
        } else {
            Duration::from_secs(1)
        };

        match rx.recv_timeout(timeout) {
            Ok(cmd) => {
                if ctx
                    .as_ref()
                    .map_or(true, |context| context.device_changed())
                {
                    ctx = create_audio_context();
                }

                let result = ctx
                    .as_mut()
                    .map_or(Err("无音频设备".to_string()), |context| {
                        context.play(&cmd)
                    });

                if result.is_err() {
                    ctx = create_audio_context();
                    if let Some(context) = ctx.as_mut() {
                        let _ = context.play(&cmd);
                    }
                }

                idle_since = None;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let should_release = match ctx.as_mut() {
            Some(context) => {
                context.remove_finished_sinks();
                if context.is_idle() {
                    match idle_since {
                        Some(since) => since.elapsed() >= AUDIO_IDLE_RELEASE_DELAY,
                        None => {
                            idle_since = Some(Instant::now());
                            false
                        }
                    }
                } else {
                    idle_since = None;
                    false
                }
            }
            None => false,
        };

        if should_release {
            ctx = None;
            idle_since = None;
        }
    }
}

fn play_file(handle: &OutputStreamHandle, path: &PathBuf, volume: f32) -> Result<Sink, String> {
    let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;
    let file = File::open(path).map_err(|e| format!("打开文件失败: {}", e))?;
    let source = Decoder::new(BufReader::new(file)).map_err(|e| format!("解码失败: {}", e))?;

    sink.set_volume(volume);
    sink.append(source);
    Ok(sink)
}

fn play_bytes(
    handle: &OutputStreamHandle,
    bytes: &'static [u8],
    volume: f32,
) -> Result<Sink, String> {
    let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;
    let source = Decoder::new(Cursor::new(bytes)).map_err(|e| format!("解码失败: {}", e))?;

    sink.set_volume(volume);
    sink.append(source);
    Ok(sink)
}

fn beep_samples(frequency: f32, duration_ms: u64, sample_rate: u32) -> Vec<f32> {
    let duration_samples = ((sample_rate as f64 * duration_ms as f64) / 1000.0) as usize;
    let two_pi_freq = 2.0 * std::f32::consts::PI * frequency;
    let sample_rate_f = sample_rate as f32;

    (0..duration_samples)
        .map(|i| (two_pi_freq * i as f32 / sample_rate_f).sin())
        .collect()
}

fn play_beep(
    handle: &OutputStreamHandle,
    frequency: f32,
    duration_ms: u64,
    volume: f32,
) -> Result<Sink, String> {
    let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;

    let sample_rate = 44100u32;
    let samples = beep_samples(frequency, duration_ms, sample_rate);

    let source = rodio::buffer::SamplesBuffer::new(1, sample_rate, samples);
    sink.set_volume(volume);
    sink.append(source);
    Ok(sink)
}

#[inline]
fn send_command(cmd: SoundCommand) {
    let _ = SOUND_SENDER.send(cmd);
}

pub struct SoundPlayer;

impl SoundPlayer {
    #[inline]
    pub fn play(path: impl AsRef<std::path::Path>, volume: f32) {
        send_command(SoundCommand::PlayFile(path.as_ref().to_path_buf(), volume));
    }

    #[inline]
    pub fn play_bytes(bytes: &'static [u8], volume: f32) {
        send_command(SoundCommand::PlayBytes(bytes, volume));
    }

    #[inline]
    pub fn play_beep(frequency: f32, duration_ms: u64, volume: f32) {
        send_command(SoundCommand::PlayBeep(frequency, duration_ms, volume));
    }
}

pub struct AppSounds;

impl AppSounds {
    // 复制音效 - 成功时播放
    pub fn play_copy_on_success() {
        let settings = crate::get_settings();
        if settings.copy_sound_timing != "success" {
            return;
        }
        Self::do_play_copy(&settings);
    }

    // 复制音效 - 立即播放
    pub fn play_copy_immediate() {
        let settings = crate::get_settings();
        if settings.copy_sound_timing != "immediate" {
            return;
        }

        if is_immediate_copy_sound_suppressed(current_time_ms()) {
            return;
        }

        Self::do_play_copy(&settings);
    }

    fn do_play_copy(settings: &crate::services::AppSettings) {
        if !settings.sound_enabled {
            return;
        }

        let volume = (settings.sound_volume / 100.0) as f32;

        if !settings.copy_sound_path.is_empty() {
            let path = Self::resolve_path(&settings.copy_sound_path);
            if path.exists() {
                SoundPlayer::play(path, volume);
                return;
            }
        }

        SoundPlayer::play_bytes(BUILTIN_COPY_SOUND, volume);
    }

    // 粘贴音效 - 成功时播放
    pub fn play_paste_on_success() {
        let settings = crate::get_settings();
        if settings.paste_sound_timing != "success" {
            return;
        }

        LAST_PASTE_SOUND_TIME_MS.store(current_time_ms(), Ordering::Relaxed);

        Self::do_play_paste(&settings);
    }

    // 粘贴音效 - 立即播放
    pub fn play_paste_immediate() {
        let settings = crate::get_settings();
        if settings.paste_sound_timing != "immediate" {
            return;
        }

        LAST_PASTE_SOUND_TIME_MS.store(current_time_ms(), Ordering::Relaxed);

        Self::do_play_paste(&settings);
    }

    fn do_play_paste(settings: &crate::services::AppSettings) {
        if !settings.sound_enabled {
            return;
        }

        let volume = (settings.sound_volume / 100.0) as f32;

        if !settings.paste_sound_path.is_empty() {
            let path = Self::resolve_path(&settings.paste_sound_path);
            if path.exists() {
                SoundPlayer::play(path, volume);
                return;
            }
        }

        SoundPlayer::play_bytes(BUILTIN_PASTE_SOUND, volume);
    }

    pub fn play_copy() {
        let settings = crate::get_settings();
        Self::do_play_copy(&settings);
    }

    pub fn play_paste() {
        let settings = crate::get_settings();
        Self::do_play_paste(&settings);
    }

    pub fn play_scroll() {
        let settings = crate::get_settings();
        if !settings.sound_enabled || !settings.quickpaste_scroll_sound {
            return;
        }

        let volume = (settings.sound_volume / 100.0) as f32;

        if !settings.quickpaste_scroll_sound_path.is_empty() {
            let path = Self::resolve_path(&settings.quickpaste_scroll_sound_path);
            if path.exists() {
                SoundPlayer::play(path, volume);
                return;
            }
        }

        SoundPlayer::play_bytes(BUILTIN_SCROLL_SOUND, volume);
    }

    fn resolve_path(path: &str) -> PathBuf {
        let p = std::path::Path::new(path);

        if p.is_absolute() {
            return p.to_path_buf();
        }

        crate::get_data_directory()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::Ordering as AtomicOrdering;

    // LAST_PASTE_SOUND_TIME_MS 是进程级全局量，测试间串行化避免互相干扰
    static SOUND_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn current_time_ms_is_recent_epoch_millis() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let t = current_time_ms();
        assert!(t > 1_000_000_000_000, "应远大于 2001 年以来的毫秒数");
        assert!(t <= now_ms, "时钟不能指向未来");
        assert!(now_ms - t < 5000, "测量误差应在 5 秒内");
    }

    #[test]
    fn mark_paste_operation_records_current_timestamp() {
        let _guard = SOUND_TEST_LOCK.lock();
        LAST_PASTE_SOUND_TIME_MS.store(0, AtomicOrdering::Relaxed);
        mark_paste_operation();
        let recorded = LAST_PASTE_SOUND_TIME_MS.load(AtomicOrdering::Relaxed);
        let now_ms = current_time_ms();
        assert!(recorded > 0);
        assert!(recorded <= now_ms);
        assert!(now_ms - recorded < 1000);
    }

    #[test]
    fn copy_immediate_suppression_boundary_is_300ms() {
        let _guard = SOUND_TEST_LOCK.lock();
        let now = current_time_ms();
        // 299ms 内 → 抑制；恰好 300ms → 放行（两端边界都断言）
        LAST_PASTE_SOUND_TIME_MS.store(now - 299, AtomicOrdering::Relaxed);
        assert!(is_immediate_copy_sound_suppressed(now));
        LAST_PASTE_SOUND_TIME_MS.store(now - 300, AtomicOrdering::Relaxed);
        assert!(!is_immediate_copy_sound_suppressed(now));
        LAST_PASTE_SOUND_TIME_MS.store(now - 301, AtomicOrdering::Relaxed);
        assert!(!is_immediate_copy_sound_suppressed(now));
        // 从未播放过（0）→ 不抑制
        LAST_PASTE_SOUND_TIME_MS.store(0, AtomicOrdering::Relaxed);
        assert!(!is_immediate_copy_sound_suppressed(now));
        // 时间戳在“未来” → 饱和减法得 0 → 抑制
        LAST_PASTE_SOUND_TIME_MS.store(now + 1000, AtomicOrdering::Relaxed);
        assert!(is_immediate_copy_sound_suppressed(now));
    }

    #[test]
    fn paste_sound_timestamp_only_written_when_timing_gate_passes() {
        let _guard = SOUND_TEST_LOCK.lock();
        // 前置条件：本机为默认设置（success），此测试刻画默认设置下的时序门顺序
        assert_eq!(
            crate::get_settings().paste_sound_timing,
            "success",
            "测试前置条件：默认 paste_sound_timing 为 success"
        );
        LAST_PASTE_SOUND_TIME_MS.store(0, AtomicOrdering::Relaxed);
        // immediate 时序不满足 → 先返回，不得写入时间戳
        AppSounds::play_paste_immediate();
        assert_eq!(
            LAST_PASTE_SOUND_TIME_MS.load(AtomicOrdering::Relaxed),
            0,
            "immediate 时序未命中时不得记录粘贴时间戳"
        );
        // success 时序命中 → 写入时间戳
        AppSounds::play_paste_on_success();
        assert!(
            LAST_PASTE_SOUND_TIME_MS.load(AtomicOrdering::Relaxed) > 0,
            "success 时序应记录粘贴时间戳"
        );
    }

    #[test]
    fn resolve_path_passes_absolute_paths_through() {
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\sounds\custom.mp3")
        } else {
            PathBuf::from("/tmp/custom.mp3")
        };
        assert_eq!(
            AppSounds::resolve_path(absolute.to_str().unwrap()),
            absolute
        );
    }

    #[test]
    fn resolve_path_joins_relative_paths_with_data_directory() {
        let _env = crate::services::database::connection::test_support::TEST_ENV_LOCK.lock();
        crate::services::database::connection::test_support::ensure_isolated_settings_env();
        // 先钉数据目录本身：隔离环境（portable.flag）→ exe 旁 data 目录，而非真实用户目录
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(
            crate::services::get_data_directory().unwrap(),
            exe_dir.join("data"),
            "隔离环境数据目录必须为 <exe>/data"
        );
        let relative = "sounds/roll.mp3";
        assert_eq!(
            AppSounds::resolve_path(relative),
            exe_dir.join("data").join(relative)
        );
    }

    #[test]
    fn beep_samples_length_matches_duration() {
        assert_eq!(beep_samples(440.0, 300, 44100).len(), 13230);
        assert_eq!(beep_samples(440.0, 1000, 44100).len(), 44100);
        assert_eq!(beep_samples(440.0, 0, 44100).len(), 0);
        // 截断语义：1ms → 44.1 → 44
        assert_eq!(beep_samples(440.0, 1, 44100).len(), 44);
    }

    #[test]
    fn beep_samples_formula_is_sine_at_44100hz() {
        let samples = beep_samples(440.0, 1000, 44100);
        assert_eq!(samples.len(), 44100);
        // 首样本 sin(0) = 0
        assert!((samples[0] - 0.0).abs() < 1e-6);
        // i=11025 → t=0.25s → sin(2π*440*0.25) = sin(220π) ≈ 0
        assert!(samples[11025].abs() < 1e-4);
        // 所有样本在 [-1, 1] 内
        assert!(samples.iter().all(|s| s.abs() <= 1.0 + 1e-6));
        // 440Hz 周期 100ms：i=4410 → sin(2π*440*0.1) = sin(88π) ≈ 0
        assert!(samples[4410].abs() < 1e-4);
        // 非零点峰值采样：i=426 → sin(2π*440*426/44100) ≈ sin(1.5728) ≈ 0.99999，
        // 只有 880Hz 翻倍（sin(2π*880*426/44100)=sin(2π*8.5007)≈sin(3.146)≈0）会在此处失败。
        assert!(
            (samples[426] - 1.0).abs() < 0.01,
            "i=426 必须接近 440Hz 相位峰值，实际: {}",
            samples[426]
        );
    }

    #[test]
    fn beep_samples_truncates_fractional_duration() {
        // 35ms * 44100Hz = 1543.5 样本 → 截断为 1543（而非四舍五入 1544）
        assert_eq!(beep_samples(440.0, 35, 44100).len(), 1543);
        // 2.5ms * 44100 = 110.25 → 110
        assert_eq!(beep_samples(440.0, 2, 44100).len(), 88);
        assert_eq!(beep_samples(440.0, 3, 44100).len(), 132);
    }
}
