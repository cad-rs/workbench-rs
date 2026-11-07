//! Stable IDs. Persistence (layout, plugin manifest) only ever uses these string IDs, never the
//! GUI's internal widget IDs.

/// A generic stable string ID; serde serializes it as a plain string.
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
    /// Stable command ID, e.g. `app.undo`.
    CommandId
);
stable_id!(
    /// Stable dock panel ID.
    PanelId
);
stable_id!(
    /// Stable ID of a central content view type.
    ViewTypeId
);
stable_id!(
    /// Stable document type ID.
    DocumentTypeId
);
stable_id!(
    /// Stable Workbench module ID.
    WorkbenchId
);
stable_id!(
    /// Stable toolbar (the future RibbonBar) group ID.
    ToolbarGroupId
);

/// Runtime document instance ID (not persisted; reassigned after the layout is restored).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

/// Runtime task instance ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TaskId(pub u64);

/// Runtime central tab instance ID (not persisted; persistence uses ViewTypeId).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TabInstanceId(pub u64);
