//! 平台运行时：产品装配、命令执行、任务事件泵、上下文快照与布局持久化。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use workbench_api as api;
use api::{
    ComboState, CommandArgs, CommandCtx, CommandExecution, CommandInvocation, CommandKind,
    CommandResult, CtxSnapshot, DocSummary, Hotkey, InvocationRecord, InvocationStatus, PanelId,
    Registry, ServiceHost, TaskEvent, ViewTypeId, Workbench,
};

use crate::executor::AsyncExecutor;
use crate::platform_cmds;

/// 调用历史环形缓冲容量。
const INVOCATION_HISTORY: usize = 100;

/// Autosave and crash recovery run on the desktop only: a web page has no
/// clean exit to tell a crash from a closed tab, and its state store
/// (localStorage) holds little, and only text.
const AUTOSAVE: bool = cfg!(not(target_arch = "wasm32"));
/// Where autosaves live in the state store.
const AUTOSAVE_DIR: &str = "autosave/";
/// Written on a clean exit; missing at startup means the last session crashed.
const CLEAN_EXIT: &str = "autosave/session.clean";
/// The settings' key in the state store.
pub(crate) const SETTINGS: &str = "settings.json";

/// 平台运行时。UI 线程所有；前端在自己的定时器上调用 [`AppRuntime::frame_tick`]
/// （与视图渲染解耦：gpui 只在失效时重画，渲染驱动的泵在 UI 空闲时停转）。
pub struct AppRuntime {
    registry: Registry,
    pub host: ServiceHost,
    product_id: String,
    product_name: String,
    /// 产品身份与品牌（product.toml；未提供时为 builder 的默认值）。
    profile: api::ProductProfile,
    snapshot: CtxSnapshot,
    /// The app's own state: settings, layout, autosaves, the clean-exit marker.
    state: Arc<dyn api::StateStore>,
    /// The layout's key in `state`.
    layout_key: String,
    /// The layout as last loaded or saved, so an unchanged one isn't rewritten.
    saved_layout: Option<String>,
    /// 距上次自动保存的累计秒数。
    autosave_accum: f32,
    dispatch_depth: u32,
    /// 共享异步执行器（阶段 2：Async 命令的运行载体）。
    executor: AsyncExecutor,
    shutdown: Arc<AtomicBool>,
    /// 命令调用历史（可查询，供日志/自动化测试/诊断，design.md §16.4）。
    invocations: VecDeque<InvocationRecord>,
    invocation_seq: u64,
    /// 在途的原生对话框（后台线程执行，结果按帧排空）。
    dialog_in_flight: Option<FileDialogRequest>,
    dialog_result_rx: Option<std::sync::mpsc::Receiver<Option<PathBuf>>>,
    /// Finished file operations' continuations, run by `frame_tick`.
    file_done_tx: std::sync::mpsc::Sender<FileDone>,
    file_done_rx: std::sync::mpsc::Receiver<FileDone>,
    /// File operations started and not yet drained.
    file_ops: usize,
}

/// A finished file operation: its continuation, bound to the result.
type FileDone = Box<dyn FnOnce(&mut dyn api::AppServices) + Send>;

/// 需要弹原生对话框的 UI 动作。对话框在后台线程运行（阻塞式 rfd 调用
/// 绝不能进 UI 线程：会饿死 gpui 事件循环并触发窗口内部重入借用），
/// 结果经通道回传，由 [`AppRuntime::frame_tick`] 排水并在 UI 线程执行
/// `action`。
pub(crate) struct FileDialogRequest {
    /// 文件过滤扩展名（空 = 不过滤）。
    pub extensions: Vec<String>,
    /// 默认文件名（保存对话框）；打开对话框为 `None`。
    pub default_name: Option<String>,
    /// 拿到选择结果后于 UI 线程执行的后续动作（`None` = 取消）。
    pub action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
}

impl AppRuntime {
    // ---- 查询（供前端渲染） ----

    pub fn product_id(&self) -> &str {
        &self.product_id
    }

    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    /// 产品身份与品牌（窗口标题、ribbon logo、About 对话框共用此源）。
    pub fn profile(&self) -> &api::ProductProfile {
        &self.profile
    }

    pub fn snapshot(&self) -> &CtxSnapshot {
        &self.snapshot
    }

    pub fn command_title(&self, id: &str) -> Option<String> {
        self.registry.command(id).map(|c| c.title.clone())
    }

    /// 命令图标资源路径（未设置返回 None，UI 回退为纯文本）。
    pub fn command_icon(&self, id: &str) -> Option<String> {
        self.registry.command(id).and_then(|c| c.icon.clone())
    }

    pub fn command_hotkey_label(&self, id: &str) -> Option<String> {
        self.registry.command(id).and_then(|c| c.hotkey).map(|h| h.to_string())
    }

    /// 命令当前是否可用（依据最近一次 frame_tick 生成的快照）。
    pub fn is_enabled(&self, id: &str) -> bool {
        match self.registry.command(id) {
            Some(def) => match &def.enabled {
                Some(f) => f(&self.snapshot),
                None => true,
            },
            None => false,
        }
    }

    /// Toggle 命令当前是否选中（ribbon-bar.md §4.2：checked 态是命令系统的查询结果，
    /// Ribbon 不自维护开关状态；未注册或未声明 checked 条件恒为 false）。
    pub fn is_checked(&self, id: &str) -> bool {
        match self.registry.command(id) {
            Some(def) => match &def.checked {
                Some(f) => f(&self.snapshot),
                None => false,
            },
            None => false,
        }
    }

    /// Combo 命令当前状态（当前值 + 选项列表，ribbon-bar.md §4.2：每帧查询；
    /// 未注册或未声明 combo 查询恒为 None）。
    pub fn combo_state(&self, id: &str) -> Option<ComboState> {
        match self.registry.command(id) {
            Some(def) => def.combo.as_ref().and_then(|f| f(&self.snapshot)),
            None => None,
        }
    }

    /// 最近 N 条命令调用记录（design.md §16.4：自动化测试可查询状态）。
    pub fn recent_invocations(&self) -> impl Iterator<Item = &InvocationRecord> {
        self.invocations.iter().rev()
    }

    /// 已成功装载的插件 ID（阶段 3）。
    pub fn plugins_loaded(&self) -> &[String] {
        &self.registry.plugins_loaded
    }

    /// 被拒绝/失败的插件及其原因（阶段 3，验收 #19/#20 诊断）。
    pub fn plugins_rejected(&self) -> &[(String, String)] {
        &self.registry.plugins_rejected
    }

    /// 统一命令入口（跟踪版，design.md §10.1）：返回立即结果 + 异步任务句柄。
    pub fn execute_command_tracked(&mut self, id: &str, args: &CommandArgs) -> CommandInvocation {
        let seq = self.invocation_seq + 1;
        let result = self.execute_command(id, args);
        let record = self
            .invocations
            .iter()
            .find(|r| r.seq == seq)
            .cloned()
            .expect("调用记录已登记");
        let task_id = record.task_id;
        CommandInvocation {
            record,
            result,
            task_id,
        }
    }

    /// 注册的全部快捷键（前端做按键分发）。
    pub fn hotkeys(&self) -> Vec<(Hotkey, String)> {
        self.registry
            .commands()
            .filter_map(|c| c.hotkey.map(|h| (h, c.id.0.clone())))
            .collect()
    }

    /// Where the app's own state lives (settings, layout, autosaves).
    pub fn state(&self) -> &Arc<dyn api::StateStore> {
        &self.state
    }

    // ---- 帧驱动 ----

    /// 请求弹原生文件对话框（非阻塞）：对话框在后台线程运行，选择结果
    /// 由 [`Self::frame_tick`] 排水执行。同一时刻只允许一个在途对话框。
    pub(crate) fn request_dialog(&mut self, request: FileDialogRequest) {
        if self.dialog_in_flight.is_some() {
            self.host.log.warn("已有对话框在途，忽略新的打开/保存请求");
            return;
        }
        #[cfg(target_arch = "wasm32")]
        self.host.log.warn("Web 版还没有文件选择器");
        let rx = crate::dialog::spawn(request.extensions.clone(), request.default_name.clone());
        self.dialog_in_flight = Some(request);
        self.dialog_result_rx = Some(rx);
    }

    /// File operations started and not yet finished (their continuations run
    /// on a later [`Self::frame_tick`]).
    pub fn pending_file_ops(&self) -> usize {
        self.file_ops
    }

    /// Run a storage operation off the UI thread; `then` gets its result on
    /// the frame tick after it finishes.
    fn spawn_file_op<T: Send + 'static>(
        &mut self,
        op: api::StorageFuture<T>,
        then: api::FileThen<T>,
    ) {
        let done = self.file_done_tx.clone();
        self.file_ops += 1;
        self.executor.spawn_detached(async move {
            let result = op.await;
            let _ = done.send(Box::new(move |app: &mut dyn api::AppServices| then(app, result)));
        });
    }

    /// Run the continuations of finished file operations. Returns whether
    /// any ran (the UI needs a redraw).
    fn drain_file_ops(&mut self) -> bool {
        let mut ran = false;
        while let Ok(done) = self.file_done_rx.try_recv() {
            self.file_ops -= 1;
            done(self);
            ran = true;
        }
        ran
    }

    /// 排空在途对话框结果并在 UI 线程执行其后续动作。
    /// 返回是否取到了一个结果（UI 需要重绘）。
    fn drain_dialog_result(&mut self) -> bool {
        let Some(rx) = &self.dialog_result_rx else {
            return false;
        };
        let picked = match rx.try_recv() {
            Ok(picked) => {
                self.dialog_result_rx = None;
                picked
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // 线程异常退出且未发送：按取消处理。
                self.dialog_result_rx = None;
                None
            }
        };
        let Some(request) = self.dialog_in_flight.take() else {
            return false;
        };
        if picked.is_none() {
            self.host.log.info("已取消");
        }
        (request.action)(self, picked.map(api::Location::local));
        true
    }

    // ---- 账号登录（Phase 3：device flow）----
    // The login state machine lives on the ServiceHost (commands reach it via
    // ctx.host()); the runtime only drains phase updates each frame.

    /// Drain pending login-phase updates into `ws.account` (once per tick).
    /// 返回是否消费了阶段更新（UI 需要重绘）。
    fn drain_account_login(&mut self) -> bool {
        let Some(rx) = self.host.account_login_rx.as_ref() else {
            return false;
        };
        let mut updates = Vec::new();
        while let Ok(phase) = rx.try_recv() {
            updates.push(phase);
        }
        if updates.is_empty() {
            return false;
        }
        // A terminal phase (LoggedIn / Failed) closes the flow: drop the
        // channel so the next login can start fresh.
        let closed = updates
            .last()
            .is_some_and(|p| !matches!(p, api::AccountPhase::Pending(_)));
        if closed {
            self.host.account_login_rx = None;
            self.host.account_login_cancel = None;
        }
        if let Some(phase) = updates.pop() {
            if let api::AccountPhase::LoggedIn(user) = &phase {
                self.host.log.info(format!("已登录为 {}", user.name));
            }
            self.host.ws.account = phase;
        }
        true
    }

    /// 平台节拍：泵任务事件、自动保存计时、同步活动文档/标签标题、刷新上下文快照。
    ///
    /// 返回 `true` 表示本次节拍改动了 UI 可见状态（任务事件、对话框结果、
    /// 文件读写结果、登录阶段、自动保存、活动文档），前端据此排一次重绘。
    /// 前端在自己的定时器上驱动本方法，而不是在视图渲染里——gpui 只在失效时重画，渲染驱动的泵
    /// 会在 UI 空闲时停转，任务结果回到通道里没人取（诊断面板的后台任务计数
    /// 因此滞后）。
    pub fn frame_tick(&mut self, dt_secs: f32) -> bool {
        let mut changed = self.drain_dialog_result();
        changed |= self.drain_file_ops();
        changed |= self.drain_account_login();
        changed |= self.pump_task_events();
        // 自动保存（阶段 5：§18 生产级扩展）
        let interval = self
            .host
            .settings
            .get_f64("autosave.interval_secs", 30.0)
            .max(5.0) as f32;
        self.autosave_accum += dt_secs.max(0.0);
        if AUTOSAVE && self.autosave_accum >= interval {
            self.autosave_accum = 0.0;
            match self.autosave_now() {
                Ok(saved) => changed |= saved > 0,
                Err(e) => {
                    self.host.log.warn(format!("自动保存失败: {e}"));
                    changed = true;
                }
            }
        }
        // 活动文档跟随活动标签的绑定
        if let Some(tab) = self.host.ws.active_tab() {
            if tab.document.is_some() {
                let doc = tab.document;
                let before = self.host.docs.active_id();
                self.host.docs.set_active(doc);
                changed |= self.host.docs.active_id() != before;
            }
        }
        // 文档绑定标签的标题同步（含未保存标记）
        let bindings: Vec<(api::TabInstanceId, Option<api::DocumentId>)> = self
            .host
            .ws
            .tabs
            .iter()
            .map(|t| (t.instance_id, t.document))
            .collect();
        for (instance_id, doc) in bindings {
            if let Some(did) = doc {
                if let Some(d) = self.host.docs.get(did) {
                    let title = format!("{}{}", d.title, if d.dirty { " •" } else { "" });
                    if let Some(slot) = self
                        .host
                        .ws
                        .tabs
                        .iter_mut()
                        .find(|t| t.instance_id == instance_id)
                    {
                        changed |= slot.title != title;
                        slot.title = title;
                    }
                } else {
                    // 绑定的文档已不存在：解绑（由关闭流程一般不会发生，防御式处理）
                    if let Some(slot) = self
                        .host
                        .ws
                        .tabs
                        .iter_mut()
                        .find(|t| t.instance_id == instance_id)
                    {
                        if slot.document.is_some() {
                            slot.document = None;
                            changed = true;
                        }
                    }
                }
            }
        }
        self.snapshot = self.make_snapshot();
        // 有任务在跑时持续排帧（进度动画），任务终态事件被本节拍取走后
        // `changed` 为真，面板才会把「运行」计数落回 0。
        changed |= self.host.tasks.summary().0 > 0;
        changed
    }

    // ---- 自动保存与崩溃恢复（阶段 5：§18）----
    //
    // 自动保存是状态存储里 `autosave/` 下的成对条目：内容 `<标题>_.<ext>`
    // 与元数据 `<标题>_.json`；干净退出时写入 `autosave/session.clean`。

    /// 立即把所有脏文档序列化写入自动保存区（`autosave/<标题>_.<ext>`）。
    /// 不改动文档的 dirty/location 状态——用户保存流程不受影响。
    pub fn autosave_now(&mut self) -> Result<usize, String> {
        if !AUTOSAVE {
            return Err("Web 版暂不支持自动保存".to_string());
        }
        let dirty: Vec<(api::DocumentId, String, String)> = {
            let host = &self.host;
            host.docs
                .docs()
                .filter(|d| d.dirty)
                .map(|d| {
                    let ext = host
                        .docs
                        .type_of(&d.type_id)
                        .and_then(|t| t.extensions.first().cloned())
                        .unwrap_or_else(|| "dat".to_string());
                    (d.id, d.title.clone(), ext)
                })
                .collect()
        };
        let mut saved = 0;
        for (id, title, ext) in dirty {
            let (bytes, type_id) = {
                let host = &self.host;
                let doc = host.docs.get(id).ok_or("文档不存在")?;
                let def = host
                    .docs
                    .type_of(&doc.type_id)
                    .ok_or("未注册的文档类型")?;
                ((def.serialize)(doc.content.as_ref())?, doc.type_id.clone())
            };
            // 文件名清洗：替换路径非法字符
            let safe_title: String = title
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect();
            self.state
                .write(&format!("{AUTOSAVE_DIR}{safe_title}_.{ext}"), &bytes)?;
            // 元数据旁车：恢复时的原标题（以及真实类型，供诊断）
            let meta = serde_json::json!({ "doc_id": id.0, "type_id": type_id.0, "title": title });
            let _ = self.state.write(
                &format!("{AUTOSAVE_DIR}{safe_title}_.json"),
                meta.to_string().as_bytes(),
            );
            saved += 1;
        }
        self.host.stats.autosaves += 1;
        if saved > 0 {
            self.host.log.info(format!("自动保存：{saved} 个文档"));
        }
        Ok(saved)
    }

    /// 启动时：上次异常退出（没有干净退出标记）就恢复自动保存的文档。上次
    /// 干净退出、或恢复出了文档，都清空自动保存区——旧会话的自动保存不该在
    /// 以后的某次崩溃后冒出来；一个都没恢复成功时保留，留待排查。
    fn restore_session(&mut self) {
        let crashed = self.state.read(CLEAN_EXIT).is_none();
        let recovered = if crashed {
            self.host.log.warn("[会话] 上次未正常退出，尝试恢复自动保存…");
            self.recover_autosaves()
        } else {
            0
        };
        if !crashed || recovered > 0 {
            for key in self.state.keys(AUTOSAVE_DIR) {
                self.state.remove(&key);
            }
        }
    }

    /// 重新打开自动保存区里的文档（标记为未保存），返回恢复的个数。
    fn recover_autosaves(&mut self) -> usize {
        let keys = self.state.keys(AUTOSAVE_DIR);
        let mut recovered = 0;
        for (meta_key, stem) in keys.iter().filter_map(|k| Some((k, k.strip_suffix(".json")?))) {
            // 同名内容条目，扩展名即文档类型的扩展名
            let content = keys.iter().find_map(|key| {
                let ext = key.strip_prefix(stem)?.strip_prefix('.')?;
                (ext != "json").then_some((key, ext))
            });
            let Some((content_key, ext)) = content else { continue };
            let Some(meta) = self
                .state
                .read(meta_key)
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                continue;
            };
            let name = stem.strip_prefix(AUTOSAVE_DIR).unwrap_or(stem);
            let title = format!(
                "[恢复] {}",
                meta.get("title").and_then(|t| t.as_str()).unwrap_or(name)
            );
            // The recovered document gets no file: its first save asks where
            // to put it.
            let restored = self
                .host
                .docs
                .find_type_by_extension(ext)
                .map(|def| def.id.clone())
                .ok_or_else(|| format!("没有登记 .{ext} 扩展名的文档类型"))
                .and_then(|type_id| {
                    let bytes = self.state.read(content_key).ok_or("内容已不在")?;
                    self.host.docs.open_bytes(&type_id, &title, None, &bytes)
                });
            match restored {
                Ok(id) => {
                    if let Some(d) = self.host.docs.get_mut(id) {
                        d.dirty = true; // 恢复内容必须由用户确认后保存
                    }
                    self.host.docs.set_active(Some(id));
                    recovered += 1;
                }
                Err(e) => self
                    .host
                    .log
                    .warn(format!("自动保存恢复失败 {content_key}: {e}")),
            }
        }
        if recovered > 0 {
            self.host.log.warn(format!(
                "检测到上次异常退出，已恢复 {recovered} 个自动保存文档（标记为未保存，请确认后保存）"
            ));
        }
        recovered
    }

    // ---- 帧统计（阶段 5：诊断面板）----

    /// GUI 前端每帧调用。
    pub fn record_frame(&mut self, dt_ms: f32) {
        self.host.record_frame(dt_ms);
    }

    /// 泵任务事件通道。返回是否取到了事件（任务状态/日志/调用历史变化，
    /// 前端需要重绘）。
    fn pump_task_events(&mut self) -> bool {
        let rx = self.host.task_events();
        let events: Vec<TaskEvent> = {
            let guard = rx.lock().unwrap_or_else(|p| p.into_inner());
            std::iter::from_fn(|| guard.try_recv().ok()).collect()
        };
        let any = !events.is_empty();
        for ev in events {
            self.host.tasks.apply_event(&ev);
            match ev {
                TaskEvent::Progress { .. } => {}
                TaskEvent::Log { message, .. } => {
                    self.host.log.info(message);
                }
                TaskEvent::Finished { outcome, id } => {
                    // 推进命令调用历史中的对应记录（统一调用结果，阶段 2）
                    let (status, note) = match &outcome {
                        api::TaskOutcome::Succeeded { note, commit } => (
                            InvocationStatus::Succeeded,
                            commit.as_ref().map(|c| c.label.clone()).or_else(|| note.clone()),
                        ),
                        api::TaskOutcome::Failed { error } => {
                            (InvocationStatus::Failed, Some(error.clone()))
                        }
                        api::TaskOutcome::Cancelled => (InvocationStatus::Cancelled, None),
                    };
                    if let Some(r) = self
                        .invocations
                        .iter_mut()
                        .rev()
                        .find(|r| r.task_id == Some(id))
                    {
                        r.status = status;
                        r.note = note;
                    }
                    match outcome {
                        api::TaskOutcome::Succeeded { note, commit } => {
                            if let Some(commit) = commit {
                                // 后台任务结果在事务边界提交：文档版本校验（design.md §10.5）
                                let label = commit.label.clone();
                                match self.host.docs.apply_commit(commit) {
                                    Ok(_) => self
                                        .host
                                        .log
                                        .info(format!("后台任务提交「{label}」成功")),
                                    Err(e) => self.host.log.warn(e),
                                }
                            }
                            self.host.log.info(format!(
                                "任务完成{}",
                                note.map(|n| format!("：{n}")).unwrap_or_default()
                            ));
                        }
                        api::TaskOutcome::Failed { error } => {
                            self.host.log.error(format!("任务失败：{error}"));
                        }
                        api::TaskOutcome::Cancelled => {
                            self.host.log.info("任务已取消");
                        }
                    }
                }
            }
        }
        any
    }

    // ---- 命令执行 ----

    /// 登记/更新一条调用记录。
    fn track(&mut self, record: InvocationRecord) {
        self.invocations.push_back(record);
        while self.invocations.len() > INVOCATION_HISTORY {
            self.invocations.pop_front();
        }
    }

    fn note_invocation(&mut self, seq: u64, status: InvocationStatus, note: Option<String>) {
        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
            r.status = status;
            r.note = note;
        }
    }

    /// 统一命令入口（design.md §10.6）。同时登记可查询的调用历史。
    pub fn execute_command(&mut self, id: &str, args: &CommandArgs) -> CommandResult {
        // 登记调用（先于执行；异步/后台保持 Running，由任务终态事件推进）
        self.invocation_seq += 1;
        let seq = self.invocation_seq;
        let execution = self.registry.command(id).map(|d| d.execution());
        self.track(InvocationRecord {
            seq,
            command: id.to_string(),
            execution,
            status: InvocationStatus::Running,
            task_id: None,
            note: None,
        });

        if self.dispatch_depth > 16 {
            let msg = "命令嵌套过深（可能存在自递归）".to_string();
            self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
            self.host.log.error(msg.clone());
            return CommandResult::failed(msg);
        }
        // Account commands run on the host directly (they manage the
        // background login thread; same interception pattern as dialogs).
        match id {
            crate::platform_cmds::ACCOUNT_LOGIN => {
                let result = match self.host.begin_account_login() {
                    Ok(()) => CommandResult::done(),
                    Err(e) => CommandResult::failed(e),
                };
                let status = if result.is_ok() {
                    InvocationStatus::Succeeded
                } else {
                    InvocationStatus::Failed
                };
                self.note_invocation(seq, status, None);
                return result;
            }
            crate::platform_cmds::ACCOUNT_CANCEL_LOGIN => {
                self.host.cancel_account_login();
                self.note_invocation(seq, InvocationStatus::Succeeded, None);
                return CommandResult::done();
            }
            crate::platform_cmds::ACCOUNT_LOGOUT => {
                self.host.account_logout();
                self.note_invocation(seq, InvocationStatus::Succeeded, None);
                return CommandResult::done();
            }
            _ => {}
        }
        let Some(mut def) = self.registry.take_command(id) else {
            let msg = format!("未注册的命令: {id}");
            self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
            self.host.log.error(msg.clone());
            return CommandResult::failed(msg);
        };
        let snapshot = self.make_snapshot();
        if let Some(enabled) = &def.enabled {
            if !enabled(&snapshot) {
                self.registry.put_command(def);
                let msg = format!("命令 {id} 当前上下文不可用");
                self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
                self.host.log.warn(msg.clone());
                return CommandResult::failed(msg);
            }
        }
        let result = match &mut def.kind {
            CommandKind::Sync(handler) => {
                self.dispatch_depth += 1;
                let r = handler(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices), args);
                self.dispatch_depth -= 1;
                r
            }
            CommandKind::Async(starter) => {
                self.dispatch_depth += 1;
                let started = starter(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices));
                self.dispatch_depth -= 1;
                match started {
                    Ok(spec) => {
                        // 任务会话（进度/取消）+ 共享执行器驱动 future
                        let session =
                            self.host
                                .tasks
                                .begin(&spec.title, spec.cancellable, api::TaskKind::Async);
                        let job = (spec.job)(api::TaskCtx::new(
                            session.handle.clone(),
                            spec.input,
                        ));
                        let tx = self.host.tasks.event_sender();
                        let task_id = session.id;
                        self.executor.spawn_detached(async move {
                            let outcome = match job.await {
                                Ok(commit) => api::TaskOutcome::Succeeded {
                                    note: None,
                                    commit,
                                },
                                Err(api::TaskFailure::Cancelled) => api::TaskOutcome::Cancelled,
                                Err(api::TaskFailure::Error(e)) => {
                                    api::TaskOutcome::Failed { error: e }
                                }
                            };
                            let _ = tx.send(api::TaskEvent::Finished { id: task_id, outcome });
                        });
                        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
                            r.task_id = Some(task_id);
                        }
                        self.host.log.info(format!(
                            "已提交异步任务 #{}「{}」",
                            task_id.0, spec.title
                        ));
                        CommandResult::done_with(format!("异步任务 #{}", task_id.0))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            }
            CommandKind::Background(starter) => {
                self.dispatch_depth += 1;
                let started = starter(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices));
                self.dispatch_depth -= 1;
                match started {
                    Ok(spec) => {
                        let title = spec.title.clone();
                        let task_id = self.host.tasks.spawn(spec);
                        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
                            r.task_id = Some(task_id);
                        }
                        self.host.log.info(format!(
                            "已提交后台任务 #{}「{title}」",
                            task_id.0
                        ));
                        CommandResult::done_with(format!("后台任务 #{}", task_id.0))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            }
        };
        self.registry.put_command(def);
        // 同步命令立即到终态；异步/后台命令在任务完成事件中推进
        match (&result, &execution) {
            (CommandResult::Done { note }, Some(CommandExecution::Sync)) => {
                self.note_invocation(seq, InvocationStatus::Succeeded, note.clone());
            }
            (CommandResult::Failed { error }, Some(CommandExecution::Sync)) => {
                self.note_invocation(seq, InvocationStatus::Failed, Some(error.clone()));
            }
            (CommandResult::Failed { error }, _) => {
                // 异步/后台提交失败（启动器返回 Err）直接终态
                self.note_invocation(seq, InvocationStatus::Failed, Some(error.clone()));
            }
            _ => {}
        }
        if let CommandResult::Failed { error } = &result {
            self.host.log.error(format!("命令 {id} 失败：{error}"));
        }
        result
    }

    fn make_snapshot(&self) -> CtxSnapshot {
        let active_doc_id = self
            .host
            .ws
            .active_tab()
            .and_then(|t| t.document)
            .or(self.host.docs.active_id());
        let active_document = active_doc_id.and_then(|id| {
            self.host.docs.get(id).map(|d| DocSummary {
                id: d.id,
                title: d.title.clone(),
                dirty: d.dirty,
                revision: d.revision,
                can_undo: self.host.docs.can_undo(id),
                can_redo: self.host.docs.can_redo(id),
            })
        });
        let active_view_type = self.host.ws.active_tab().map(|t| t.type_id.0.clone());
        CtxSnapshot {
            active_document,
            active_view_type,
            work_mode: self.host.ws.work_mode.clone(),
        }
    }

    // ---- 布局持久化（design.md §6.5） ----

    /// 保存布局；`ui` 为 GUI 前端私有物理布局数据，`dock` 为停靠区物理布局
    /// （前端 `DockArea::dump` 的序列化结果）。返回是否写入：与上次加载或
    /// 保存的布局相同时不重写（Web 端定期调用）。
    pub fn save_layout(
        &mut self,
        ui: serde_json::Value,
        dock: Option<serde_json::Value>,
    ) -> Result<bool, String> {
        let mut file = self.host.ws.save_layout();
        file.ui = Some(ui);
        file.dock = dock;
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        if self.saved_layout.as_deref() == Some(json.as_str()) {
            return Ok(false);
        }
        let written = self.state.write(&self.layout_key, json.as_bytes());
        // Remembered even when the write failed: a failing store is retried
        // when the layout changes, not on every call.
        self.saved_layout = Some(json);
        written.map(|()| true)
    }

    /// 尝试加载布局：应用核心部分（面板/标签/模式），返回前端私有部分供适配器恢复。
    /// 未注册的面板/视图被跳过并记录诊断（验收 #9）。
    pub fn load_layout(&mut self) -> Option<RestoredLayout> {
        let text = String::from_utf8(self.state.read(&self.layout_key)?).ok()?;
        let file: api::LayoutFile = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                self.host
                    .log
                    .warn(format!("布局文件解析失败，使用默认布局: {e}"));
                return None;
            }
        };
        let skipped = self.host.ws.apply_layout(&file);
        for msg in skipped {
            self.host.log.warn(msg);
        }
        self.saved_layout = Some(text);
        self.snapshot = self.make_snapshot();
        self.host.log.info("已恢复上次的工作区布局");
        Some(RestoredLayout {
            ui: file.ui,
            dock: file.dock,
        })
    }
}

/// [`AppRuntime::load_layout`] 恢复出的前端私有布局部分。
pub struct RestoredLayout {
    /// GUI 前端私有物理布局（历史字段；前端自行解释）。
    pub ui: Option<serde_json::Value>,
    /// 停靠区物理布局（gpui 前端交给 `DockArea::load`）。
    pub dock: Option<serde_json::Value>,
}

impl api::AppServices for AppRuntime {
    fn host(&mut self) -> &mut ServiceHost {
        &mut self.host
    }

    fn snapshot(&self) -> CtxSnapshot {
        self.snapshot.clone()
    }

    fn dispatch(&mut self, command: &str, args: &CommandArgs) -> CommandResult {
        self.execute_command(command, args)
    }

    fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String> {
        let doc = if bind_document {
            self.host.docs.active_id()
        } else {
            None
        };
        let type_id = ViewTypeId::new(view_type);
        self.host.ws.open_view(&type_id, doc, None)?;
        self.snapshot = self.make_snapshot();
        Ok(())
    }

    fn close_active_view(&mut self) -> Result<(), String> {
        let index = self.host.ws.active_tab;
        match self.host.ws.close_tab(index) {
            Some(_) => {
                self.host.log.info(format!("已关闭中央标签 #{}", index + 1));
                self.snapshot = self.make_snapshot();
                Ok(())
            }
            None => Err("没有可关闭的中央标签".to_string()),
        }
    }

    fn set_work_mode(&mut self, mode: &str) -> Result<(), String> {
        self.host.ws.set_work_mode(mode.to_string());
        self.host.log.info(format!("切换工作模式 → {mode}"));
        self.snapshot = self.make_snapshot();
        Ok(())
    }

    fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String> {
        let now_visible = self.host.ws.toggle_panel(&PanelId::new(panel_id))?;
        self.host.log.info(format!(
            "面板 `{panel_id}` {}",
            if now_visible { "已显示" } else { "已隐藏" }
        ));
        Ok(())
    }

    fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        self.disable_plugin(plugin_id)
    }

    fn autosave_now(&mut self) -> Result<usize, String> {
        self.autosave_now()
    }

    fn request_file_dialog(
        &mut self,
        extensions: Vec<String>,
        default_name: Option<String>,
        action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
    ) {
        self.request_dialog(FileDialogRequest {
            extensions,
            default_name,
            action,
        });
    }

    fn read_file(&mut self, location: api::Location, then: api::FileThen<Vec<u8>>) {
        let read = self.host.storage.read(&location);
        self.spawn_file_op(read, then);
    }

    fn write_file(&mut self, location: api::Location, bytes: Vec<u8>, then: api::FileThen<()>) {
        let write = self.host.storage.write(&location, bytes);
        self.spawn_file_op(write, then);
    }
}

impl AppRuntime {
    /// 运行时禁用插件（阶段 5，§16.2/§18）：移除它的全部贡献——命令、视图
    /// 类型、面板、文档类型、Ribbon 组与 Tab，更新插件注册镜像。不可恢复
    /// （如需重载请重启应用）。
    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        if !self.registry.plugins_loaded.iter().any(|x| x == plugin_id) {
            return Err(format!("插件未装载: {plugin_id}"));
        }
        let removed = self.registry.remove_from_source(plugin_id);
        self.host.ws.remove_commands(&removed, plugin_id);
        self.host.ws.remove_from_source(plugin_id);
        self.registry.plugins_loaded.retain(|x| x != plugin_id);
        self.registry.plugins_disabled.push(plugin_id.to_string());
        if let Some(entry) = self
            .host
            .plugins
            .iter_mut()
            .find(|p| p.id == plugin_id && p.state == "Loaded")
        {
            entry.state = "Disabled";
            entry.detail = Some("运行时禁用".to_string());
        }
        self.host.log.warn(format!(
            "插件 {plugin_id} 已禁用，移除 {} 条命令",
            removed.len()
        ));
        Ok(removed)
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        // 通知异步执行器工作线程退出
        self.shutdown.store(true, Ordering::Relaxed);
        // 会话干净退出标记（阶段 5：崩溃恢复），下次启动由 `restore_session` 检查。
        if AUTOSAVE {
            let _ = self.state.write(CLEAN_EXIT, b"ok");
        }
        let _ = self.host.settings.save();
    }
}

// ---- 产品装配 ----

/// 插件装载阶段：装配期由 builder 调用，向注册表贡献命令/工具栏/服务。
/// 失败通过 `Registry::error`/`plugins_rejected` 记录，不中断装配。
pub type PluginStage = Box<dyn FnOnce(&mut Registry) + Send>;

/// 产品装配器（design.md §13）：选择 Workbench 模块、默认工作模式与初始布局。
pub struct WorkbenchAppBuilder {
    product_id: String,
    product_name: String,
    /// 产品身份与品牌（product.toml）。提供时覆盖 product_id/product_name，
    /// 并作为窗口标题、ribbon logo、About 对话框的数据源。
    profile: Option<api::ProductProfile>,
    workbenches: Vec<Box<dyn Workbench>>,
    plugin_stages: Vec<PluginStage>,
    default_mode: String,
    work_modes: Vec<String>,
    layout_path: Option<PathBuf>,
    open_on_start: Vec<(String, bool)>,
    create_default_document: bool,
    /// A document file to open at startup (the file-association entry
    /// point); applied after the default document would be created.
    open_file_on_start: Option<PathBuf>,
    /// 默认文档类型 ID（多文档类型时由产品指定）。
    default_doc_type: Option<String>,
    smoke_seconds: Option<f32>,
    /// 产品资源源（SVG 图标等）；None 时只有平台标准资源。
    assets: Option<std::sync::Arc<dyn api::AssetSource>>,
    /// GUI 后端标签（诊断面板/日志用；产品按构建选定填写）。
    backend_label: String,
    /// 产品声明的额外 GPU 设备需求（不透明传递；前端解释具体类型）。
    gpu_requirements: Option<Box<dyn std::any::Any>>,
    /// Ribbon 折叠能力开关（ribbon-bar.md §4.5）；默认 false。
    ribbon_collapse_enabled: bool,
    /// UI 主题名（"dark"/"light"）；写入 settings 默认值，用户设置优先。
    theme: String,
    /// 产品的模块解析器：给定插件声明的模块 ID，返回要装配的 Workbench。
    ///
    /// 插件是运行期单位——它启用产品已编译的模块，而不是携带代码。产品提供
    /// 这张 `模块 ID -> Workbench` 表，平台在插件装载之后调用它。
    module_loader: Option<ModuleLoader>,
    /// Injected account capability (Phase 3); None = no login surface.
    account: Option<Arc<dyn api::AccountService>>,
    /// Where user files live: the local file system on the desktop, plus
    /// whatever the product adds.
    storage: api::Storage,
}

/// 产品的模块解析器：给定插件 ID 与它本次要装配的模块 ID，返回 Workbench。
///
/// 按插件逐个调用，这样平台能用 [`api::Registry::with_source`] 给该插件的
/// 全部贡献盖上来源，运行期禁用才能精确移除。
pub type ModuleLoader = Box<dyn Fn(&str, &[String]) -> Vec<Box<dyn Workbench>> + Send>;

impl WorkbenchAppBuilder {
    pub fn new(product_id: impl Into<String>, product_name: impl Into<String>) -> Self {
        #[allow(unused_mut)]
        let mut storage = api::Storage::default();
        #[cfg(not(target_arch = "wasm32"))]
        storage.register(api::storage::LOCAL, Arc::new(crate::local_files::LocalFiles));
        Self {
            product_id: product_id.into(),
            product_name: product_name.into(),
            profile: None,
            workbenches: Vec::new(),
            plugin_stages: Vec::new(),
            default_mode: "常规".to_string(),
            work_modes: vec!["常规".to_string()],
            layout_path: None,
            open_on_start: Vec::new(),
            create_default_document: false,
            open_file_on_start: None,
            default_doc_type: None,
            smoke_seconds: None,
            assets: None,
            backend_label: "unknown".to_string(),
            gpu_requirements: None,
            ribbon_collapse_enabled: false,
            theme: "dark".to_string(),
            module_loader: None,
            account: None,
            storage,
        }
    }

    /// Add a place user files can live (the browser's origin-private file
    /// system, a cloud store, …), under `id`: documents there have
    /// [`api::Location`]s with that provider. The desktop has the local file
    /// system as [`api::storage::LOCAL`]; registering that id replaces it.
    pub fn with_storage(
        mut self,
        id: impl Into<String>,
        provider: Arc<dyn api::StorageProvider>,
    ) -> Self {
        self.storage.register(id, provider);
        self
    }

    /// 设置产品的模块解析器（插件启用的 Workbench 模块由它装配）。
    pub fn with_module_loader(mut self, loader: ModuleLoader) -> Self {
        self.module_loader = Some(loader);
        self
    }

    /// 设置产品身份与品牌（`product.toml` 加载结果，见 [`crate::product`]）。
    /// profile 的 id/name 覆盖 [`WorkbenchAppBuilder::new`] 的对应参数。
    pub fn with_profile(mut self, profile: api::ProductProfile) -> Self {
        self.product_id = profile.id.clone();
        self.product_name = profile.name.clone();
        self.profile = Some(profile);
        self
    }

    /// Inject the account capability (Phase 3). None (the default) hides the
    /// login surface; the reference implementation is
    /// [`crate::account::DeviceFlowClient`].
    pub fn with_account(mut self, service: Arc<dyn api::AccountService>) -> Self {
        self.account = Some(service);
        self
    }

    /// 设置产品资源源（SVG 图标等静态资源）。查询顺序：产品源 → 平台标准源
    /// （[`crate::assets::standard_assets`]），产品可用同名路径覆盖标准图标。
    pub fn with_assets(
        mut self,
        source: impl api::AssetSource + 'static,
    ) -> Self {
        self.assets = Some(std::sync::Arc::new(source));
        self
    }

    /// 生效的资源源：产品源优先，平台标准源兜底。
    pub fn asset_source(&self) -> std::sync::Arc<dyn api::AssetSource> {
        let mut sources: Vec<std::sync::Arc<dyn api::AssetSource>> = Vec::new();
        if let Some(a) = &self.assets {
            sources.push(a.clone());
        }
        sources.push(std::sync::Arc::new(*crate::assets::standard_assets()));
        std::sync::Arc::new(api::ChainedAssets::new(sources))
    }

    /// 声明产品额外需要的 GPU 设备需求（features / limits），在前端创建设备前
    /// 落位。类型由 GUI 前端解释（workbench-ui 期望
    /// `workbench_ui::gpu_view::GpuRequirements`），本层只透传 `Box<dyn Any>`
    /// （GUI 无关契约不变）。典型用途：产品自己的渲染器需要高于平台基线的
    /// 限制（如计算着色器的 storage buffer 数）。
    pub fn with_gpu_requirements(mut self, requirements: Box<dyn std::any::Any>) -> Self {
        self.gpu_requirements = Some(requirements);
        self
    }

    /// 取出 GPU 设备需求（`build()` 之前的装配步骤消费）。
    pub fn take_gpu_requirements(&mut self) -> Option<Box<dyn std::any::Any>> {
        self.gpu_requirements.take()
    }

    /// 标记 GUI 后端（诊断面板/自动化查询用）。
    pub fn with_backend_label(mut self, label: impl Into<String>) -> Self {
        self.backend_label = label.into();
        self
    }

    /// Enable the ribbon collapse feature (ribbon-bar.md §4.5): the toggle
    /// button, Tab double-click and Ctrl+F1 become active.
    pub fn ribbon_collapse_enabled(mut self, on: bool) -> Self {
        self.ribbon_collapse_enabled = on;
        self
    }

    /// UI 主题（`"dark"` / `"light"`）。作为 settings `ui.theme` 的默认值——
    /// 用户已保存的设置优先。
    pub fn with_theme(mut self, name: impl Into<String>) -> Self {
        self.theme = name.into();
        self
    }

    pub fn with_workbench(mut self, wb: impl Workbench) -> Self {
        self.workbenches.push(Box::new(wb));
        self
    }

    /// 追加一个插件装载阶段（如 `workbench_rhai::plugin_stage(dirs)`）。
    pub fn with_plugin_stage(mut self, stage: PluginStage) -> Self {
        self.plugin_stages.push(stage);
        self
    }

    /// 工作模式列表与默认模式。
    pub fn with_modes(mut self, modes: &[&str], default: &str) -> Self {
        self.work_modes = modes.iter().map(|s| s.to_string()).collect();
        self.default_mode = default.to_string();
        self
    }

    /// The layout file. On the desktop its directory holds the rest of the
    /// app's state too (settings, autosaves); the default is the product's
    /// config directory. The web keeps state in localStorage and uses only
    /// the file name.
    pub fn with_layout_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.layout_path = Some(path.into());
        self
    }

    /// 启动时打开的中央视图（视图类型 ID，是否绑定文档）。
    pub fn open_on_start(mut self, view_type: &str, bind_document: bool) -> Self {
        self.open_on_start.push((view_type.to_string(), bind_document));
        self
    }

    /// 启动时创建一个默认文档（首个注册的文档类型）。
    pub fn create_default_document(mut self, on: bool) -> Self {
        self.create_default_document = on;
        self
    }

    /// Open this document file at startup (the file-association entry
    /// point). It is read in the background and opens on an early frame
    /// tick, after the default document, so a launch with both ends up with
    /// the launched file active. A read failure is logged, not fatal.
    pub fn with_open_file_on_start(mut self, path: PathBuf) -> Self {
        self.open_file_on_start = Some(path);
        self
    }

    /// 指定默认文档类型（产品注册了多个文档类型时用）。
    ///
    /// 未指定时回落到第一个注册的文档类型。
    pub fn with_default_document_type(mut self, type_id: impl Into<String>) -> Self {
        self.default_doc_type = Some(type_id.into());
        self
    }

    /// 冒烟模式：窗口打开 `seconds` 秒后自动退出（自动化验证用）。
    pub fn smoke(mut self, seconds: f32) -> Self {
        self.smoke_seconds = Some(seconds);
        self
    }

    pub fn smoke_seconds(&self) -> Option<f32> {
        self.smoke_seconds
    }

    /// 装配产品。Workbench 初始化失败被捕获隔离（验收 #19）。
    pub fn build(mut self) -> Result<AppRuntime, String> {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let (tx, rx) = api::task_channel();
        let mut host = ServiceHost::new(tx, rx, self.default_mode.clone());
        host.ws.set_work_modes(self.work_modes.clone(), &self.default_mode);

        let mut registry = Registry::new();
        for wb in &self.workbenches {
            registry.workbenches.push(wb.id());
            let result = catch_unwind(AssertUnwindSafe(|| wb.init(&mut registry)));
            if let Err(panic) = result {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "未知 panic".to_string());
                registry.error(format!("Workbench `{}` 初始化失败: {msg}", wb.id()));
            }
        }

        // 插件装载阶段（Rhai 脚本插件等）：与 Workbench 同样的隔离策略（验收 #19）
        for stage in self.plugin_stages {
            let result = catch_unwind(AssertUnwindSafe(|| stage(&mut registry)));
            if let Err(panic) = result {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "未知 panic".to_string());
                registry.error(format!("插件装载阶段失败: {msg}"));
            }
        }

        // 插件启用的 Workbench 模块：插件声明的是 ID，产品用模块解析器把它
        // 们变成真正的 Workbench。放在平台命令之前，让“窗口”组能收到这些
        // 模块注册的面板。
        if let Some(loader) = self.module_loader.take() {
            let mut loaded: Vec<String> = Vec::new();
            for (plugin_id, modules) in registry.plugin_modules.clone() {
                let pending: Vec<String> = modules
                    .iter()
                    .filter(|id| !loaded.contains(id))
                    .cloned()
                    .collect();
                if pending.is_empty() {
                    continue;
                }
                loaded.extend(pending.iter().cloned());
                // Stamp every contribution these modules register with the
                // plugin id, so disabling the plugin can remove them.
                let workbenches = loader(&plugin_id, &pending);
                registry.with_source(&plugin_id, |reg| {
                    for wb in workbenches {
                        reg.workbenches.push(wb.id());
                        let result = catch_unwind(AssertUnwindSafe(|| wb.init(reg)));
                        if let Err(panic) = result {
                            let msg = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_else(|| "未知 panic".to_string());
                            reg.error(format!("插件模块初始化失败: {msg}"));
                        }
                    }
                });
            }
        }

        // 平台命令（含每个面板的显示/隐藏命令与“窗口”工具栏组）
        let panel_ids: Vec<(String, String)> = registry
            .panels
            .iter()
            .map(|p| (p.id.0.clone(), p.title.clone()))
            .collect();
        platform_cmds::register(&mut registry, &panel_ids);

        // 注册表 → 运行时
        for def in std::mem::take(&mut registry.doc_types) {
            if let Err(e) = host.docs.register_type(def) {
                host.log.error(e);
            }
        }
        for def in std::mem::take(&mut registry.views) {
            if let Err(e) = host.ws.register_view(def) {
                host.log.error(e);
            }
        }
        for def in std::mem::take(&mut registry.panels) {
            if let Err(e) = host.ws.register_panel(def, true) {
                host.log.error(e);
            }
        }
        // Ribbon Tab（显式）先行注册；遗留工具栏组落入默认 Tab「主页」
        for tab in std::mem::take(&mut registry.ribbon) {
            host.ws.add_ribbon_tab(tab);
        }
        for group in std::mem::take(&mut registry.toolbar) {
            host.ws.add_toolbar_group(group);
        }
        // QAT 快速访问（产品静态集，ribbon-bar.md §4.1）
        host.ws.qat = std::mem::take(&mut registry.qat);
        // Ribbon 折叠能力开关（产品经 builder 声明，ribbon-bar.md §4.5）
        host.ws.ribbon_collapse_enabled = self.ribbon_collapse_enabled;
        for (ty, value) in registry.take_services() {
            host.insert_raw(ty, value);
        }
        // Workbench 建议的工作模式
        if !registry.work_modes.is_empty() {
            host.ws
                .set_work_modes(registry.work_modes.clone(), &registry.default_mode);
        }
        // 产品指定的默认文档类型（多文档类型时用）
        if let Some(id) = self.default_doc_type.clone() {
            host.ws.default_document_type = Some(id);
        }
        for err in &registry.errors {
            host.log.error(format!("[装配] {err}"));
        }
        for note in &registry.notes {
            host.log.info(format!("[装配] {note}"));
        }
        for plugin in &registry.plugins_loaded {
            host.log.info(format!("[插件] 已装载 {plugin}"));
        }
        for (id, reason) in &registry.plugins_rejected {
            host.log.error(format!("[插件] 拒绝加载 {id}：{reason}"));
        }

        // 状态存储：布局 + 设置 + 自动保存（阶段 5）
        let (state, layout_key) =
            crate::state::open(&self.product_id, self.layout_path.as_deref());
        let (settings, settings_err) = api::Settings::load(state.clone(), SETTINGS);
        host.settings = settings;
        if let Some(e) = settings_err {
            host.log.warn(format!("[设置] {e}"));
        }
        // UI 主题默认值：builder 声明，用户已保存的设置优先。
        if host.settings.get("ui.theme").is_none() {
            let _ = host
                .settings
                .set("ui.theme", serde_json::Value::String(self.theme.clone()));
        }
        // Brand accent: product.toml's branding.accent seeds ui.accent when
        // the product ships one and the user has not customized it.
        if let Some(accent) = &self.profile.as_ref().and_then(|p| p.accent.as_ref()) {
            if host.settings.get("ui.accent").is_none() {
                let _ = host
                    .settings
                    .set("ui.accent", serde_json::Value::String(accent.to_string()));
            }
        }
        // Setting specs: registry (platform built-ins + plugins) → host, so
        // the settings panel and `app.settings.set` see one list. Defaults
        // that are not yet stored are seeded (in memory only — a fresh value
        // persists on first write).
        host.setting_specs = std::mem::take(&mut registry.setting_specs);
        for spec in &host.setting_specs {
            if host.settings.get(&spec.key).is_none() {
                let default = spec.effective_default();
                host.settings.seed(&spec.key, default);
            }
        }
        // Locale tables: registry (platform + plugins) → host localizer.
        for (locale, key, text) in std::mem::take(&mut registry.locale_entries) {
            host.localizer.add_table(&locale, [(key, text)]);
        }
        // Default locale from product.toml [i18n]; the user's saved choice wins.
        if let Some(locale) = self.profile.as_ref().and_then(|p| p.default_locale.as_ref()) {
            if host.settings.get("ui.locale").is_none() {
                host.settings
                    .seed("ui.locale", serde_json::Value::String(locale.clone()));
            }
        }
        host.stats.backend = self.backend_label.clone();
        // Account capability (Phase 3): injected by the product; commands and
        // the status bar render from host.account + ws.account.
        host.account = self.account;
        host.storage = self.storage;

        // 插件注册镜像（诊断/管理面板用）；tier 为清单声明的 license 要求。
        for id in &registry.plugins_loaded {
            let tier = registry
                .plugin_tiers
                .iter()
                .find(|(pid, _)| pid == id)
                .map(|(_, tier)| *tier);
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Loaded",
                detail: None,
                tier,
            });
        }
        for (id, reason) in &registry.plugins_rejected {
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Rejected",
                detail: Some(reason.clone()),
                tier: None,
            });
        }

        // 共享异步执行器（Async 命令的运行载体）
        let shutdown = Arc::new(AtomicBool::new(false));
        let executor = AsyncExecutor::new(2, shutdown.clone());
        let (file_done_tx, file_done_rx) = std::sync::mpsc::channel();

        let mut runtime = AppRuntime {
            registry,
            host,
            product_id: self.product_id.clone(),
            product_name: self.product_name.clone(),
            profile: self
                .profile
                .clone()
                .unwrap_or_else(|| api::ProductProfile {
                    id: self.product_id.clone(),
                    name: self.product_name.clone(),
                    ..Default::default()
                }),
            snapshot: CtxSnapshot::default(),
            state,
            layout_key,
            saved_layout: None,
            autosave_accum: 0.0,
            dispatch_depth: 0,
            executor,
            shutdown,
            invocations: VecDeque::new(),
            invocation_seq: 0,
            dialog_in_flight: None,
            dialog_result_rx: None,
            file_done_tx,
            file_done_rx,
            file_ops: 0,
        };
        runtime.snapshot = runtime.make_snapshot();

        // 崩溃恢复：上次会话无 clean 标记 → 恢复自动保存文档（阶段 5）。
        if AUTOSAVE {
            runtime.restore_session();
        }

        // 默认文档
        if self.create_default_document {
            // The product names its default type when it has more than one;
            // otherwise the only registered type is used.
            let default_type = self
                .default_doc_type
                .as_ref()
                .and_then(|id| {
                    runtime
                        .host
                        .docs
                        .types()
                        .into_iter()
                        .find(|t| &t.id.0 == id)
                        .map(|t| t.id.clone())
                })
                .or_else(|| runtime.host.docs.types().first().map(|t| t.id.clone()));
            if let Some(t) = default_type {
                let id = runtime
                    .host
                    .docs
                    .create(&t, "未命名")
                    .map_err(|e| format!("创建默认文档失败: {e}"))?;
                runtime.host.docs.set_active(Some(id));
            }
        }

        // 启动时打开文件（文件关联入口；失败只记日志，不阻断启动）。
        if let Some(path) = self.open_file_on_start.clone() {
            let mut ctx = api::CommandCtx::new(&mut runtime);
            crate::platform_cmds::open_document(&mut ctx, api::Location::local(path));
        }

        // 初始视图（布局文件已存在时由适配器负责恢复布局，不再叠加初始视图）
        if runtime.state.read(&runtime.layout_key).is_none() {
            for (vt, bind) in &self.open_on_start {
                if let Err(e) = api::AppServices::open_view(&mut runtime, vt, *bind) {
                    runtime.host.log.warn(format!("打开初始视图 {vt} 失败: {e}"));
                }
            }
        }
        Ok(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> AppRuntime {
        WorkbenchAppBuilder::new("test.dialog", "Dialog test")
            .with_layout_path(std::env::temp_dir().join("workbench-rs-dialog-test/layout.json"))
            .build()
            .expect("装配失败")
    }

    fn request(
        action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
    ) -> FileDialogRequest {
        FileDialogRequest {
            extensions: vec!["rcad".to_string()],
            default_name: None,
            action,
        }
    }

    /// A module-requested pick is drained by `frame_tick`, which runs the
    /// action on the UI thread with the picked path. The blocking OS modal
    /// loop itself never runs on the UI thread.
    #[test]
    fn 对话框请求_由帧节拍排水并在主线程执行动作() {
        let mut app = runtime();
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Option<api::Location>>();
        tx.send(Some(PathBuf::from("C:/tmp/part.rcad"))).unwrap();
        app.dialog_in_flight = Some(request(Box::new(move |_, picked| {
            let _ = done_tx.send(picked);
        })));
        app.dialog_result_rx = Some(rx);

        assert!(app.frame_tick(0.0), "取到结果应报告需要重绘");
        assert_eq!(
            done_rx.try_recv().unwrap(),
            Some(api::Location::local("C:/tmp/part.rcad"))
        );
        assert!(app.dialog_in_flight.is_none());
        assert!(app.dialog_result_rx.is_none());
    }

    /// A cancelled dialog (user closed the picker) still runs the action with
    /// `None`, so the module can log the cancellation.
    #[test]
    fn 对话框取消_动作收到空值() {
        let mut app = runtime();
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Option<api::Location>>();
        tx.send(None).unwrap();
        app.dialog_in_flight = Some(request(Box::new(move |_, picked| {
            let _ = done_tx.send(picked);
        })));
        app.dialog_result_rx = Some(rx);

        assert!(app.frame_tick(0.0));
        assert_eq!(done_rx.try_recv().unwrap(), None);
    }

    /// A picker thread that dies without sending is treated as a cancellation
    /// (the action still runs, with `None`).
    #[test]
    fn 对话框线程死亡_按取消处理() {
        let mut app = runtime();
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Option<api::Location>>();
        drop(tx);
        app.dialog_in_flight = Some(request(Box::new(move |_, picked| {
            let _ = done_tx.send(picked);
        })));
        app.dialog_result_rx = Some(rx);

        assert!(app.frame_tick(0.0));
        assert_eq!(done_rx.try_recv().unwrap(), None);
    }

    /// Only one dialog may be in flight: a second request while one is pending
    /// is ignored (and must not replace the pending request or its channel).
    #[test]
    fn 在途对话框期间_新请求被忽略() {
        let mut app = runtime();
        app.dialog_in_flight = Some(FileDialogRequest {
            extensions: vec![],
            default_name: None,
            action: Box::new(|_, _| {}),
        });

        app.request_dialog(request(Box::new(|_, _| {})));

        let pending = app.dialog_in_flight.as_ref().expect("在途请求应保留");
        assert!(pending.extensions.is_empty(), "在途请求被替换了");
        assert!(
            app.dialog_result_rx.is_none(),
            "被忽略的请求不应安装结果通道"
        );
    }
}
