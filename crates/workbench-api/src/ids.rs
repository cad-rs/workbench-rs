//! 稳定 ID。持久化（布局、插件清单）只使用这些字符串 ID，绝不使用 GUI 内部控件 ID。

/// 通用稳定字符串 ID，serde 序列化为纯字符串。
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct StableId(pub String);

impl StableId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for StableId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
impl From<String> for StableId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for StableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for StableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}`", self.0)
    }
}

macro_rules! stable_id {
    ($(#$meta:tt)* $name:ident) => {
        $(#$meta)*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name {
            pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl From<&str> for $name {
            fn from(s: &str) -> Self { Self(s.to_string()) }
        }
        impl From<String> for $name {
            fn from(s: String) -> Self { Self(s) }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "`{}`", self.0) }
        }
    };
}

stable_id!(
    /// 命令稳定 ID，例如 `app.undo`。
    CommandId
);
stable_id!(
    /// Dock Panel 稳定 ID。
    PanelId
);
stable_id!(
    /// 中央内容视图类型稳定 ID。
    ViewTypeId
);
stable_id!(
    /// 文档类型稳定 ID。
    DocumentTypeId
);
stable_id!(
    /// Workbench 模块稳定 ID。
    WorkbenchId
);
stable_id!(
    /// 工具栏（未来的 RibbonBar）Group 稳定 ID。
    ToolbarGroupId
);

/// 运行期文档实例 ID（不持久化；布局恢复后重新分配）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

/// 运行期任务实例 ID。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TaskId(pub u64);

/// 运行期中央标签实例 ID（不持久化；持久化使用 ViewTypeId）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TabInstanceId(pub u64);
