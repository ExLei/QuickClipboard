// 测试夹具：文件名/目录刻意包含 "tao-"、"event_loop"、"runner.rs"，
// 使此处触发的 panic 命中 startup_diagnostics::is_known_tao_reentrant_panic 的
// 位置匹配规则（生产环境中的 tao 框架 panic 位置形如 …/tao-0.30.0/src/event_loop/runner.rs:NNN）。
// 仅通过 include! 在测试中引入，不参与模块编译。
pub fn trigger_tao_like_panic() {
    panic!("either event handler is re-entrant");
}
