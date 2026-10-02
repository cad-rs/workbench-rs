//! Python Host API v1（design.md §11.3/§11.4）。
//!
//! 插件通过 `register(api)` 收到的 `api` 对象即 [`HostApi`]。为避免在 Python
//! 回调期间持有注册表借用，所有请求先收集到 [`Collected`]，回调返回后由
//! 加载器统一应用到 `Registry`（重复 ID、来源标记等校验走平台既有路径）。

use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::Bound;

use crate::manifest::HOST_API_VERSION;

// 插件取消异常：task.check_cancelled() 在已取消时抛出。
pyo3::create_exception!(workbench, PluginCancelled, PyException);

/// 一条插件注册的命令请求。
pub struct PyCommandReg {
    pub id: String,
    pub title: String,
    pub callback: Py<PyAny>,
    pub background: bool,
}

/// 插件在 register 期间提交的全部请求。
#[derive(Default)]
pub struct Collected {
    pub commands: Vec<PyCommandReg>,
    /// (group_id, command_id, label)
    pub toolbar: Vec<(String, String, Option<String>)>,
    pub notes: Vec<String>,
}

/// Python 侧的宿主 API 对象（传给插件的 register(api)）。
#[pyclass]
pub struct HostApi {
    pub plugin_id: String,
    pub collected: Arc<Mutex<Collected>>,
}

#[pymethods]
impl HostApi {
    /// 注册命令。`background=True` 时 callback 签名为 `(task, args)`，
    /// 在后台任务线程执行（task 提供 report/check_cancelled/log）；
    /// 否则 callback 签名为 `(args)`，在 UI 线程同步执行，可返回字符串备注。
    #[pyo3(signature = (id, title, callback, background=false))]
    fn register_command(
        &self,
        id: &str,
        title: &str,
        callback: Py<PyAny>,
        background: bool,
    ) -> PyResult<()> {
        if id.trim().is_empty() {
            return Err(PyException::new_err("命令 id 不能为空"));
        }
        let mut c = self.collected.lock().unwrap_or_else(|p| p.into_inner());
        if c.commands.iter().any(|cmd| cmd.id == id) {
            return Err(PyException::new_err(format!("命令重复注册: {id}")));
        }
        c.commands.push(PyCommandReg {
            id: id.to_string(),
            title: title.to_string(),
            callback,
            background,
        });
        Ok(())
    }

    /// 向工具栏贡献条目；group 不存在时自动创建（组 ID 建议带插件前缀）。
    #[pyo3(signature = (group, command, label=None))]
    fn add_toolbar_item(
        &self,
        group: &str,
        command: &str,
        label: Option<&str>,
    ) -> PyResult<()> {
        if group.trim().is_empty() {
            return Err(PyException::new_err("工具栏组 id 不能为空"));
        }
        let mut c = self.collected.lock().unwrap_or_else(|p| p.into_inner());
        c.toolbar
            .push((group.to_string(), command.to_string(), label.map(|s| s.to_string())));
        Ok(())
    }

    /// 记录一条装配期提示（出现在平台日志）。
    fn log_info(&self, message: &str) {
        let mut c = self.collected.lock().unwrap_or_else(|p| p.into_inner());
        c.notes.push(format!("[{}] {}", self.plugin_id, message));
    }

    /// 宿主 API 版本。
    #[getter]
    fn api_version(&self) -> &str {
        HOST_API_VERSION
    }
}

/// 传给插件后台回调的 task 对象（design.md §11.4 task.set_title/report/check_cancelled 的对等物）。
#[pyclass]
pub struct PyTask {
    pub handle: workbench_api::ProgressHandle,
}

#[pymethods]
impl PyTask {
    /// 报告进度（fraction: None 表示不确定进度）。
    #[pyo3(signature = (fraction=None, stage=None))]
    fn report(&self, fraction: Option<f64>, stage: Option<&str>) {
        self.handle
            .report(fraction.map(|f| f as f32), stage);
    }
    fn set_stage(&self, stage: &str) {
        self.handle.report(None, Some(stage));
    }
    fn log(&self, message: &str) {
        self.handle.log(message);
    }
    /// 已取消时抛出 PluginCancelled，插件应尽快返回。
    fn check_cancelled(&self) -> PyResult<()> {
        self.handle
            .check_cancelled()
            .map(|_| ())
            .map_err(|_| PluginCancelled::new_err("任务已取消"))
    }
    fn is_cancelled(&self) -> bool {
        self.handle.is_cancelled()
    }
}

/// 把 CommandArgs 转为 Python dict。
pub fn args_to_dict<'py>(py: Python<'py>, args: &workbench_api::CommandArgs) -> Bound<'py, PyDict> {
    let dict = PyDict::new(py);
    for (k, v) in &args.values {
        let _ = dict.set_item(k, v);
    }
    dict
}
