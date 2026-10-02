//! 平台运行时：产品装配、命令执行、任务事件泵、上下文快照与布局持久化。

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use workbench_api as api;
use api::{
    CommandArgs, CommandCtx, CommandExecution, CommandInvocation, CommandKind, CommandResult,
    CtxSnapshot, DocSummary, Hotkey, InvocationRecord, InvocationStatus, PanelId, Registry,
    ServiceHost, TaskEvent, ViewTypeId, Workbench,
};

use crate::executor::AsyncExecutor;
use crate::platform_cmds;

/// 调用历史环形缓冲容量。
const INVOCATION_HISTORY: usize = 100;

/// 平台运行时。UI 线程所有；前端每帧调用 [`AppRuntime::frame_tick`]。
pub struct AppRuntime {
    registry: Registry,
    pub host: ServiceHost,
    product_id: String,
    product_name: String,
    snapshot: CtxSnapshot,
    layout_path: PathBuf,
    /// 自动保存目录（`<配置目录>/autosave`）。
    autosave_dir: PathBuf,
    /// 距上次自动保存的累计秒数。
    autosave_accum: f32,
    /// 绑定文档的视图类型（用于布局恢复时重新绑定默认文档）。
    doc_bound_types: HashSet<String>,
    dispatch_depth: u32,
    /// 共享异步执行器（阶段 2：Async 命令的运行载体）。
    executor: AsyncExecutor,
    shutdown: Arc<AtomicBool>,
    /// 命令调用历史（可查询，供日志/自动化测试/诊断，design.md §16.4）。
    invocations: VecDeque<InvocationRecord>,
    invocation_seq: u64,
}

impl AppRuntime {
    // ---- 查询（供前端渲染） ----

    pub fn product_id(&self) -> &str {
        &self.product_id
    }

    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    pub fn snapshot(&self) -> &CtxSnapshot {
        &self.snapshot
    }

    pub fn command_title(&self, id: &str) -> Option<String> {
        self.registry.command(id).map(|c| c.title.clone())
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

    pub fn layout_path(&self) -> &PathBuf {
        &self.layout_path
    }

    // ---- 帧驱动 ----

    /// 每帧调用：泵任务事件、自动保存计时、同步活动文档/标签标题、刷新上下文快照。
    pub fn frame_tick(&mut self, dt_secs: f32) {
        self.pump_task_events();
        // 自动保存（阶段 5：§18 生产级扩展）
        let interval = self
            .host
            .settings
            .get_f64("autosave.interval_secs", 30.0)
            .max(5.0) as f32;
        self.autosave_accum += dt_secs.max(0.0);
        if self.autosave_accum >= interval {
            self.autosave_accum = 0.0;
            if let Err(e) = self.autosave_now() {
                self.host.log.warn(format!("自动保存失败: {e}"));
            }
        }
        // 活动文档跟随活动标签的绑定
        if let Some(tab) = self.host.ws.active_tab() {
            if tab.document.is_some() {
                let doc = tab.document;
                self.host.docs.set_active(doc);
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
                        slot.document = None;
                    }
                }
            }
        }
        self.snapshot = self.make_snapshot();
    }

    // ---- 自动保存与崩溃恢复（阶段 5：§18）----

    /// 立即把所有脏文档序列化写入自动保存目录（`<id>_<标题>.<ext>`）。
    /// 不改动文档的 dirty/path 状态——用户保存流程不受影响。
    pub fn autosave_now(&mut self) -> Result<usize, String> {
        let _ = std::fs::create_dir_all(&self.autosave_dir);
        let dirty: Vec<(api::DocumentId, String, String)> = {
            let host = &self.host;
            host.docs
                .docs()
                .filter(|d| d.dirty)
                .filter_map(|d| {
                    let ext = host
                        .docs
                        .type_of(&d.type_id)
                        .and_then(|t| t.extensions.first().cloned())
                        .unwrap_or_else(|| "dat".to_string());
                    Some((d.id, d.title.clone(), ext))
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
            let file = self.autosave_dir.join(format!("{}_.{}", safe_title, ext));
            std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
            // 恢复时按扩展名匹配文档类型，因此需要记录真实类型；用旁车 JSON 记录
            let meta = serde_json::json!({ "doc_id": id.0, "type_id": type_id.0, "title": title });
            let _ = std::fs::write(
                self.autosave_dir.join(format!("{}_.json", safe_title)),
                meta.to_string(),
            );
            saved += 1;
        }
        self.host.stats.autosaves += 1;
        if saved > 0 {
            self.host.log.info(format!("自动保存：{saved} 个文档"));
        }
        Ok(saved)
    }

    /// 启动恢复：检测上次异常退出（clean 标记缺失）时，从自动保存目录恢复文档。
    /// 自动保存产物为成对文件：`<标题>.<ext>`（内容）+ `<标题>.json`(元数据)。
    /// 返回恢复的文档标题列表。
    fn recover_autosaves(&mut self) -> Vec<String> {
        let marker = self.autosave_marker_path();
        let mut recovered = Vec::new();
        let entries = match std::fs::read_dir(&self.autosave_dir) {
            Ok(e) => e,
            Err(_) => return recovered,
        };
        let mut meta_files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        meta_files.sort();
        for meta_path in meta_files {
            let stem = match meta_path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                Some(s) => s,
                None => continue,
            };
            let meta: serde_json::Value = match std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
            {
                Some(m) => m,
                None => continue,
            };
            // 找同 stem 的内容文件（扩展名即文档类型扩展名，open_file 按它匹配类型）
            let doc_file = match std::fs::read_dir(&self.autosave_dir) {
                Ok(entries) => entries
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| {
                        p.extension().map(|x| x == "json") != Some(true)
                            && p.file_stem().map(|s| s == stem.as_str()).unwrap_or(false)
                    }),
                Err(_) => None,
            };
            let Some(doc_file) = doc_file else { continue };
            match self.host.docs.open_file(&doc_file) {
                Ok(id) => {
                    let title = format!(
                        "[恢复] {}",
                        meta.get("title").and_then(|t| t.as_str()).unwrap_or(&stem)
                    );
                    if let Some(d) = self.host.docs.get_mut(id) {
                        d.title = title.clone();
                        d.dirty = true; // 恢复内容必须由用户确认后保存
                    }
                    self.host.docs.set_active(Some(id));
                    recovered.push(title);
                }
                Err(e) => self
                    .host
                    .log
                    .warn(format!("自动保存恢复失败 {doc_file:?}: {e}")),
            }
        }
        if !recovered.is_empty() {
            self.host.log.warn(format!(
                "检测到上次异常退出，已恢复 {} 个自动保存文档（标记为未保存，请确认后保存）",
                recovered.len()
            ));
        }
        // 恢复完成后（或上次干净退出）清理自动保存目录，避免重复恢复
        if marker.exists() || !recovered.is_empty() {
            for file in std::fs::read_dir(&self.autosave_dir)
                .into_iter()
                .flatten()
                .flatten()
            {
                let _ = std::fs::remove_file(file.path());
            }
        }
        recovered
    }

    /// 会话干净退出标记路径。
    fn autosave_marker_path(&self) -> PathBuf {
        self.autosave_dir.join("session.clean")
    }

    // ---- 帧统计（阶段 5：诊断面板）----

    /// GUI 前端每帧调用。
    pub fn record_frame(&mut self, dt_ms: f32) {
        self.host.record_frame(dt_ms);
    }

    fn pump_task_events(&mut self) {
        let rx = self.host.task_events();
        let events: Vec<TaskEvent> = {
            let guard = rx.lock().unwrap_or_else(|p| p.into_inner());
            std::iter::from_fn(|| guard.try_recv().ok()).collect()
        };
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
                can_undo: d.can_undo(),
                can_redo: d.can_redo(),
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

    /// 保存布局；`ui` 为 GUI 前端私有物理布局数据。
    pub fn save_layout(&self, ui: serde_json::Value) -> Result<PathBuf, String> {
        let mut file = self.host.ws.save_layout();
        file.ui = Some(ui);
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        if let Some(dir) = self.layout_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.layout_path, json)
            .map_err(|e| format!("写入布局 {} 失败: {e}", self.layout_path.display()))?;
        Ok(self.layout_path.clone())
    }

    /// 尝试加载布局：应用核心部分（面板/标签/模式），返回前端私有部分供适配器恢复。
    /// 未注册的面板/视图被跳过并记录诊断（验收 #9）。
    pub fn load_layout(&mut self) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(&self.layout_path).ok()?;
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
        // 重新绑定文档视图到默认文档（文档不跨会话持久化）
        let default_doc = self.host.docs.active_id();
        for tab in self.host.ws.tabs.iter_mut() {
            if tab.document.is_none() && self.doc_bound_types.contains(&tab.type_id.0) {
                tab.document = default_doc;
            }
        }
        self.host.log.info("已恢复上次的工作区布局");
        file.ui
    }
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
}

impl AppRuntime {
    /// 运行时禁用插件（阶段 5，§16.2/§18）：移除其命令、Ribbon 组与 Tab，
    /// 更新插件注册镜像。不可恢复（如需重载请重启应用）。
    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        if !self.registry.plugins_loaded.iter().any(|x| x == plugin_id) {
            return Err(format!("插件未装载: {plugin_id}"));
        }
        let removed = self.registry.remove_commands_from_source(plugin_id);
        self.host.ws.remove_commands(&removed, plugin_id);
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
        // 会话干净退出标记（阶段 5：崩溃恢复）
        if let Some(dir) = self.layout_path.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("session.clean"), b"ok");
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
    workbenches: Vec<Box<dyn Workbench>>,
    plugin_stages: Vec<PluginStage>,
    default_mode: String,
    work_modes: Vec<String>,
    layout_path: Option<PathBuf>,
    open_on_start: Vec<(String, bool)>,
    create_default_document: bool,
    smoke_seconds: Option<f32>,
    /// GUI 后端标签（诊断面板/日志用；产品按构建选定填写）。
    backend_label: String,
}

impl WorkbenchAppBuilder {
    pub fn new(product_id: impl Into<String>, product_name: impl Into<String>) -> Self {
        Self {
            product_id: product_id.into(),
            product_name: product_name.into(),
            workbenches: Vec::new(),
            plugin_stages: Vec::new(),
            default_mode: "常规".to_string(),
            work_modes: vec!["常规".to_string()],
            layout_path: None,
            open_on_start: Vec::new(),
            create_default_document: false,
            smoke_seconds: None,
            backend_label: "unknown".to_string(),
        }
    }

    /// 标记 GUI 后端（诊断面板/自动化查询用）。
    pub fn with_backend_label(mut self, label: impl Into<String>) -> Self {
        self.backend_label = label.into();
        self
    }

    pub fn with_workbench(mut self, wb: impl Workbench) -> Self {
        self.workbenches.push(Box::new(wb));
        self
    }

    /// 追加一个插件装载阶段（如 `workbench_python::plugin_stage(dirs)`）。
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

    /// 冒烟模式：窗口打开 `seconds` 秒后自动退出（自动化验证用）。
    pub fn smoke(mut self, seconds: f32) -> Self {
        self.smoke_seconds = Some(seconds);
        self
    }

    pub fn smoke_seconds(&self) -> Option<f32> {
        self.smoke_seconds
    }

    /// 装配产品。Workbench 初始化失败被捕获隔离（验收 #19）。
    pub fn build(self) -> Result<AppRuntime, String> {
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

        // 插件装载阶段（Python 插件等）：与 Workbench 同样的隔离策略（验收 #19）
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

        // 平台命令（含每个面板的显示/隐藏命令与“窗口”工具栏组）
        let panel_ids: Vec<(String, String)> = registry
            .panels
            .iter()
            .map(|p| (p.id.0.clone(), p.title.clone()))
            .collect();
        platform_cmds::register(&mut registry, &panel_ids);

        // 注册表 → 运行时
        let mut doc_bound_types = HashSet::new();
        for def in std::mem::take(&mut registry.doc_types) {
            if let Some(vt) = &def.open_view {
                doc_bound_types.insert(vt.0.clone());
            }
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
        for (ty, value) in registry.take_services() {
            host.insert_raw(ty, value);
        }
        // Workbench 建议的工作模式
        if !registry.work_modes.is_empty() {
            host.ws
                .set_work_modes(registry.work_modes.clone(), &registry.default_mode);
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

        // 布局路径 + 设置 + 自动保存目录（阶段 5）
        let layout_path = self.layout_path.clone().unwrap_or_else(|| {
            default_layout_dir(&self.product_id).join("layout.json")
        });
        let config_dir = layout_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let autosave_dir = config_dir.join("autosave");
        let (settings, settings_err) = api::Settings::load(&config_dir.join("settings.json"));
        host.settings = settings;
        if let Some(e) = settings_err {
            host.log.warn(format!("[设置] {e}"));
        }
        host.stats.backend = self.backend_label.clone();

        // 插件注册镜像（诊断/管理面板用）
        for id in &registry.plugins_loaded {
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Loaded",
                detail: None,
            });
        }
        for (id, reason) in &registry.plugins_rejected {
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Rejected",
                detail: Some(reason.clone()),
            });
        }

        // 共享异步执行器（Async 命令的运行载体）
        let shutdown = Arc::new(AtomicBool::new(false));
        let executor = AsyncExecutor::new(2, shutdown.clone());

        let mut runtime = AppRuntime {
            registry,
            host,
            product_id: self.product_id.clone(),
            product_name: self.product_name.clone(),
            snapshot: CtxSnapshot::default(),
            layout_path,
            autosave_dir,
            autosave_accum: 0.0,
            doc_bound_types,
            dispatch_depth: 0,
            executor,
            shutdown,
            invocations: VecDeque::new(),
            invocation_seq: 0,
        };
        runtime.snapshot = runtime.make_snapshot();

        // 崩溃恢复：上次会话无 clean 标记 → 恢复自动保存文档（阶段 5）
        let marker = runtime.autosave_marker_path();
        let crashed = !marker.exists();
        if crashed {
            runtime.host.log.warn("[会话] 上次未正常退出，尝试恢复自动保存…");
            runtime.recover_autosaves();
        }
        // 删除标记：本会话结束时由 Drop 重新写入
        let _ = std::fs::remove_file(&marker);

        // 默认文档
        if self.create_default_document {
            if let Some(t) = runtime.host.docs.types().first().map(|t| t.id.clone()) {
                let id = runtime
                    .host
                    .docs
                    .create(&t, "未命名")
                    .map_err(|e| format!("创建默认文档失败: {e}"))?;
                runtime.host.docs.set_active(Some(id));
            }
        }

        // 初始视图（布局文件已存在时由适配器负责恢复布局，不再叠加初始视图）
        if !runtime.layout_path.exists() {
            for (vt, bind) in &self.open_on_start {
                if let Err(e) = api::AppServices::open_view(&mut runtime, vt, *bind) {
                    runtime.host.log.warn(format!("打开初始视图 {vt} 失败: {e}"));
                }
            }
        }
        Ok(runtime)
    }
}

/// 默认布局目录：Windows 用 %APPDATA%/<product_id>，其他平台 ~/.config/<product_id>。
fn default_layout_dir(product_id: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(product_id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config").join(product_id))
            .unwrap_or_else(std::env::temp_dir)
    }
}
