//! 平台服务聚合与工作台可见的能力门面（design.md §4.1 Platform）。

use std::any::{Any, TypeId};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::command::{CommandArgs, CommandResult, CtxSnapshot};
use crate::documents::DocumentService;
use crate::log::LogStore;
use crate::settings::Settings;
use crate::tasks::{TaskEventRx, TaskManager};
use crate::workspace::WorkspaceState;

/// 运行时统计（诊断面板/自动化测试用）。
#[derive(Clone, Debug, Default)]
pub struct RuntimeStats {
    /// 渲染帧数（由 GUI 前端每帧上报；无头运行恒为 0）。
    pub frames: u64,
    /// 帧耗时指数滑动平均（毫秒）。
    pub avg_frame_ms: f32,
    /// 最近一帧耗时（毫秒）。
    pub last_frame_ms: f32,
    /// GUI 后端标签（产品装配时写入，如 "egui"/"gpui"）。
    pub backend: String,
    /// 自动保存次数。
    pub autosaves: u64,
}

/// 插件注册条目（诊断/管理面板用）。
#[derive(Clone, Debug)]
pub struct PluginEntry {
    pub id: String,
    /// Loaded / Rejected / Disabled
    pub state: &'static str,
    /// Rejected/Disabled 的原因。
    pub detail: Option<String>,
    /// License tier the plugin requires (manifest `tier`); None = no
    /// requirement. The licensing milestone gates loading on this.
    pub tier: Option<crate::licensing::LicenseTier>,
}

/// 平台服务聚合。UI 线程所有。
pub struct ServiceHost {
    pub docs: DocumentService,
    pub tasks: TaskManager,
    pub ws: WorkspaceState,
    pub log: LogStore,
    pub settings: Settings,
    /// Registered setting specs (platform built-ins + plugin settings),
    /// filled at build time; the settings panel renders from these and
    /// `app.settings.set` validates against them.
    pub setting_specs: Vec<crate::settings::SettingSpec>,
    /// Merged locale tables (platform built-ins + plugin contributions),
    /// filled at build time; display sites go through [`ServiceHost::tr`].
    pub localizer: crate::i18n::Localizer,
    /// Injected account capability (Phase 3); None = no login surface.
    pub account: Option<std::sync::Arc<dyn crate::account::AccountService>>,
    /// In-flight device-login poll thread: phase updates (drained each frame
    /// into `ws.account` by the runtime) and the cancel flag.
    pub account_login_rx: Option<std::sync::mpsc::Receiver<crate::workspace::AccountPhase>>,
    pub account_login_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub stats: RuntimeStats,
    /// 插件注册表镜像（build 时填充，运行时禁用同步更新）。
    pub plugins: Vec<PluginEntry>,
    /// 平台启动时刻。
    pub started: web_time::Instant,
    extras: std::collections::HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    task_rx: TaskEventRx,
}

impl ServiceHost {
    pub fn new(
        tx: std::sync::mpsc::Sender<crate::tasks::TaskEvent>,
        task_rx: TaskEventRx,
        default_mode: impl Into<String>,
    ) -> Self {
        Self {
            docs: DocumentService::new(),
            tasks: TaskManager::new(tx),
            ws: WorkspaceState::new(default_mode),
            log: LogStore::new(1000),
            settings: Settings::in_memory(),
            setting_specs: Vec::new(),
            localizer: crate::i18n::Localizer::new(),
            account: None,
            account_login_rx: None,
            account_login_cancel: None,
            stats: RuntimeStats::default(),
            plugins: Vec::new(),
            started: web_time::Instant::now(),
            extras: std::collections::HashMap::new(),
            task_rx,
        }
    }

    /// 渲染帧上报（由 GUI 前端每帧调用）。
    pub fn record_frame(&mut self, dt_ms: f32) {
        let s = &mut self.stats;
        s.frames += 1;
        s.last_frame_ms = dt_ms;
        s.avg_frame_ms = if s.avg_frame_ms <= 0.0 {
            dt_ms
        } else {
            s.avg_frame_ms * 0.9 + dt_ms * 0.1
        };
    }

    /// 平台运行时长（秒）。
    pub fn uptime_secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// 当前 locale（`ui.locale` 设置，未设置时 `zh-CN`）。
    pub fn locale(&self) -> String {
        self.settings.get_str("ui.locale", "zh-CN")
    }

    /// 翻译一个文案键：按当前 locale 查表，找不到回退 `en`，再回退键本身。
    /// 键本身作回退值保证未翻译文案原样显示、UI 不出现空洞。
    pub fn tr(&self, key: &str) -> String {
        let locale = self.locale();
        self.localizer
            .translate(&locale, key)
            .unwrap_or(key)
            .to_string()
    }

    // ---- 账号（Phase 3：device flow，后台线程轮询）----

    /// Start the device login: `begin_login` + token polling run on a
    /// background thread (network I/O must never touch the UI thread); phase
    /// updates arrive via the channel and land in `ws.account` each frame
    /// (drained by the runtime).
    pub fn begin_account_login(&mut self) -> Result<(), String> {
        let Some(service) = self.account.clone() else {
            return Err("产品未配置账号服务".to_string());
        };
        if self.account_login_rx.is_some() {
            return Err("登录已在进行中".to_string());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = service;
            Err("Web 端登录流程尚未实现".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::AtomicBool;
            use std::sync::Arc;

            let (tx, rx) = std::sync::mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            self.account_login_rx = Some(rx);
            self.account_login_cancel = Some(cancel.clone());

            std::thread::spawn(move || {
                let req = match service.begin_login() {
                    Ok(req) => req,
                    Err(e) => {
                        let _ = tx.send(crate::workspace::AccountPhase::Failed(format!(
                            "无法开始登录: {e}"
                        )));
                        return;
                    }
                };
                let mut interval = req.interval.max(1);
                let _ = tx.send(crate::workspace::AccountPhase::Pending(req));
                loop {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(interval));
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    // RFC 8628: the token endpoint wants the device code,
                    // which the service remembers from begin_login — poll()
                    // takes no argument so a user_code can never be sent by
                    // mistake.
                    match service.poll() {
                        Ok(crate::account::PollResult::Pending) => continue,
                        Ok(crate::account::PollResult::SlowDown) => {
                            interval += crate::account::SLOW_DOWN_INTERVAL_SECS;
                            continue;
                        }
                        Ok(crate::account::PollResult::Success(user)) => {
                            let _ = tx.send(crate::workspace::AccountPhase::LoggedIn(user));
                            return;
                        }
                        Ok(crate::account::PollResult::Denied) => {
                            let _ = tx.send(crate::workspace::AccountPhase::Failed(
                                "登录被拒绝".to_string(),
                            ));
                            return;
                        }
                        Ok(crate::account::PollResult::Expired) => {
                            let _ = tx.send(crate::workspace::AccountPhase::Failed(
                                "登录码已过期".to_string(),
                            ));
                            return;
                        }
                        Err(e) => {
                            let _ = tx.send(crate::workspace::AccountPhase::Failed(format!(
                                "登录失败: {e}"
                            )));
                            return;
                        }
                    }
                }
            });
            Ok(())
        }
    }

    /// Abort an in-flight device login (the poll thread exits on the flag;
    /// the phase returns to LoggedOut).
    pub fn cancel_account_login(&mut self) {
        if let Some(flag) = self.account_login_cancel.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.account_login_rx = None;
        self.ws.account = crate::workspace::AccountPhase::LoggedOut;
    }

    /// End the session (Phase 3).
    pub fn account_logout(&mut self) {
        self.cancel_account_login();
        if let Some(service) = &self.account {
            service.logout();
        }
        self.ws.account = crate::workspace::AccountPhase::LoggedOut;
        self.log.info("已登出");
    }

    /// 取出任务事件接收端的锁（核心 frame_tick 泵事件用）。
    pub fn task_events(&self) -> Arc<Mutex<std::sync::mpsc::Receiver<crate::tasks::TaskEvent>>> {
        self.task_rx.clone()
    }

    // ---- Workbench/插件扩展服务（design.md §8.1 “服务”贡献）----

    pub fn set_service<T: Any + Send + Sync>(&mut self, value: T) {
        self.extras.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn service<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extras.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    pub fn service_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.extras.get_mut(&TypeId::of::<T>()).and_then(|b| b.downcast_mut::<T>())
    }

    /// 装配阶段的原样搬入（来自 Registry.take_services）。
    pub fn insert_raw(&mut self, ty: TypeId, value: Box<dyn Any + Send + Sync>) {
        self.extras.insert(ty, value);
    }
}

/// 工作台（Workbench 模块、脚本插件宿主、视图）可见的平台能力。
///
/// 由核心的 AppRuntime 实现；CommandCtx / ViewCtx 经由该 trait 访问平台，
/// 以保证命令与视图代码只面对稳定 API（design.md §15）。
pub trait AppServices {
    /// 平台服务聚合。
    fn host(&mut self) -> &mut ServiceHost;
    /// 当前上下文快照。
    fn snapshot(&self) -> CtxSnapshot;
    /// 按稳定 ID 执行命令（统一入口）。
    fn dispatch(&mut self, command: &str, args: &CommandArgs) -> CommandResult;
    /// 打开中央视图；`bind_document` 为 true 时绑定当前活动文档。
    fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String>;
    /// 关闭当前活动的中央标签。
    fn close_active_view(&mut self) -> Result<(), String>;
    /// 切换工作模式。
    fn set_work_mode(&mut self, mode: &str) -> Result<(), String>;
    /// 显示/隐藏面板。
    fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String>;
    /// 运行时禁用插件：移除其命令与 Ribbon 贡献（阶段 5，§16.2）。
    /// 返回被移除的命令 ID 列表。
    fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String>;
    /// 立即执行一次自动保存，返回保存的文档数（阶段 5）。
    fn autosave_now(&mut self) -> Result<usize, String>;
    /// 请求原生文件对话框（非阻塞）。
    ///
    /// 阻塞式 OS 模态循环绝不能在 UI 线程内执行：它会在 gpui 的窗口过程
    /// 重入窗口状态借用，楔死渲染循环（进程 Responding=False）。对话框因此
    /// 在后台线程运行，`action` 于下一帧节拍在 UI 线程被调用，参数为选中的
    /// 路径（`None` = 取消）。同一时刻只允许一个在途对话框。
    fn request_file_dialog(
        &mut self,
        extensions: Vec<String>,
        default_name: Option<String>,
        action: Box<dyn FnOnce(&mut dyn AppServices, Option<PathBuf>)>,
    );
}
