//! 通用文档基础设施（design.md §12.1/§12.2）。
//!
//! 平台管理文档生命周期（新建/打开/保存/脏标记/修订号/撤销历史），
//! 领域内容由 Workbench 通过 [`DocumentTypeDef`] 注册编解码，以
//! [`DocumentContent`] 类型擦除存放——平台不侵入领域数据模型。

use std::path::{Path, PathBuf};

use crate::ids::{DocumentId, DocumentTypeId};

/// 领域文档内容。通过 blanket impl，任何 `Clone + Send + Sync + 'static` 类型自动实现。
pub trait DocumentContent: std::any::Any + Send + Sync {
    fn dup(&self) -> Box<dyn DocumentContent>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl<T> DocumentContent for T
where
    T: std::any::Any + Clone + Send + Sync,
{
    fn dup(&self) -> Box<dyn DocumentContent> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// 文档类型注册信息：编解码由领域模块提供，平台不定义领域 schema。
pub struct DocumentTypeDef {
    pub id: DocumentTypeId,
    pub title: String,
    /// 关联扩展名（小写、不带点），按此匹配打开文件时的类型。
    pub extensions: Vec<String>,
    pub create_default: fn() -> Box<dyn DocumentContent>,
    pub serialize: fn(&dyn DocumentContent) -> Result<Vec<u8>, String>,
    pub deserialize: fn(&[u8]) -> Result<Box<dyn DocumentContent>, String>,
    /// 新建/打开文档时建议打开的中央视图类型（可选）。
    pub open_view: Option<crate::ids::ViewTypeId>,
}

/// 撤销历史条目（快照式）。
struct UndoEntry {
    label: String,
    before: Box<dyn DocumentContent>,
    after: Box<dyn DocumentContent>,
}

/// 单个打开的文档。
pub struct Document {
    pub id: DocumentId,
    pub type_id: DocumentTypeId,
    pub title: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// 文档版本号：每次提交（含撤销/重做）递增。后台任务据此做提交校验。
    pub revision: u64,
    pub content: Box<dyn DocumentContent>,
    history: Vec<UndoEntry>,
    /// 已应用的历史条目数（撤销栈顶位置）。
    history_pos: usize,
}

impl Document {
    pub fn can_undo(&self) -> bool {
        self.history_pos > 0
    }
    pub fn can_redo(&self) -> bool {
        self.history_pos < self.history.len()
    }
    /// 最近一条历史标签（供 UI 显示）。
    pub fn last_action_label(&self) -> Option<&str> {
        self.history.get(self.history_pos.wrapping_sub(1)).map(|e| e.label.as_str())
    }
    /// 撤销历史深度上限。
    pub const MAX_HISTORY: usize = 100;
}

/// 通用文档服务。UI 线程所有（后台任务只能通过 DocCommit 触碰文档）。
pub struct DocumentService {
    docs: Vec<Document>,
    next_id: u64,
    types: Vec<DocumentTypeDef>,
    active: Option<DocumentId>,
}

impl DocumentService {
    pub fn new() -> Self {
        Self {
            docs: Vec::new(),
            next_id: 1,
            types: Vec::new(),
            active: None,
        }
    }

    // ---- 类型注册 ----

    pub fn register_type(&mut self, def: DocumentTypeDef) -> Result<(), String> {
        if self.types.iter().any(|t| t.id == def.id) {
            return Err(format!("文档类型重复注册: {}", def.id));
        }
        self.types.push(def);
        Ok(())
    }

    pub fn types(&self) -> &[DocumentTypeDef] {
        &self.types
    }

    pub fn type_of(&self, id: &DocumentTypeId) -> Option<&DocumentTypeDef> {
        self.types.iter().find(|t| &t.id == id)
    }

    pub fn find_type_by_extension(&self, ext: &str) -> Option<&DocumentTypeDef> {
        let ext = ext.to_ascii_lowercase();
        self.types.iter().find(|t| t.extensions.iter().any(|e| *e == ext))
    }

    // ---- 查询 ----

    pub fn get(&self, id: DocumentId) -> Option<&Document> {
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.docs.iter_mut().find(|d| d.id == id)
    }

    pub fn docs(&self) -> impl Iterator<Item = &Document> {
        self.docs.iter()
    }

    pub fn active_id(&self) -> Option<DocumentId> {
        self.active
    }

    pub fn active(&self) -> Option<&Document> {
        self.active.and_then(|id| self.get(id))
    }

    pub fn set_active(&mut self, id: Option<DocumentId>) {
        self.active = id;
    }

    /// 查询文档当前修订号（提交策略与测试使用；None 表示文档不存在/已关闭）。
    pub fn revision(&self, id: DocumentId) -> Option<u64> {
        self.get(id).map(|d| d.revision)
    }

    /// 以类型化方式只读访问文档内容。
    pub fn read<T: DocumentContent, R>(
        &self,
        id: DocumentId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        self.get(id)?.content.as_any().downcast_ref::<T>().map(f)
    }

    // ---- 生命周期 ----

    pub fn create(&mut self, type_id: &DocumentTypeId, title: &str) -> Result<DocumentId, String> {
        let def = self
            .type_of(type_id)
            .ok_or_else(|| format!("未注册的文档类型: {type_id}"))?;
        let content = (def.create_default)();
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        self.docs.push(Document {
            id,
            type_id: type_id.clone(),
            title: title.to_string(),
            path: None,
            dirty: false,
            revision: 0,
            content,
            history: Vec::new(),
            history_pos: 0,
        });
        Ok(id)
    }

    /// 在事务边界提交一份新的文档内容。
    /// `undoable=false` 表示明确声明不可撤销的操作（design.md §12.2）。
    pub fn commit(
        &mut self,
        id: DocumentId,
        label: &str,
        undoable: bool,
        new_content: Box<dyn DocumentContent>,
    ) -> Result<u64, String> {
        let doc = self.get_mut(id).ok_or_else(|| format!("文档不存在: {id:?}"))?;
        if undoable {
            let before = doc.content.dup();
            doc.history.truncate(doc.history_pos);
            doc.history.push(UndoEntry {
                label: label.to_string(),
                before,
                after: new_content.dup(),
            });
            if doc.history.len() > Document::MAX_HISTORY {
                doc.history.remove(0);
            } else {
                doc.history_pos += 1;
            }
        } else {
            // 不可撤销操作使既有历史失效（避免撤销回不一致状态）。
            doc.history.clear();
            doc.history_pos = 0;
        }
        doc.content = new_content;
        doc.dirty = true;
        doc.revision += 1;
        Ok(doc.revision)
    }

    /// 类型化编辑：修改前做快照，按域模块逻辑原地修改后登记历史。
    pub fn edit<T: DocumentContent>(
        &mut self,
        id: DocumentId,
        label: &str,
        undoable: bool,
        f: impl FnOnce(&mut T),
    ) -> Result<u64, String> {
        let type_ok = self
            .get(id)
            .ok_or_else(|| format!("文档不存在: {id:?}"))?
            .content
            .as_any()
            .downcast_ref::<T>()
            .is_some();
        if !type_ok {
            return Err(format!(
                "文档内容类型不是 {}",
                std::any::type_name::<T>()
            ));
        }
        let before = self.get(id).unwrap().content.dup();
        {
            let doc = self.get_mut(id).unwrap();
            let typed = doc
                .content
                .as_any_mut()
                .downcast_mut::<T>()
                .expect("类型已校验");
            f(typed);
        }
        let after = self.get(id).unwrap().content.dup();
        let doc = self.get_mut(id).unwrap();
        if undoable {
            doc.history.truncate(doc.history_pos);
            doc.history.push(UndoEntry {
                label: label.to_string(),
                before,
                after,
            });
            if doc.history.len() > Document::MAX_HISTORY {
                doc.history.remove(0);
            } else {
                doc.history_pos += 1;
            }
        } else {
            doc.history.clear();
            doc.history_pos = 0;
        }
        doc.dirty = true;
        doc.revision += 1;
        Ok(doc.revision)
    }

    /// 应用后台任务的 DocCommit（宿主已在 UI 线程校验版本）。
    pub fn apply_commit(&mut self, commit: crate::tasks::DocCommit) -> Result<u64, String> {
        let revision = self.get(commit.doc_id).map(|d| d.revision);
        match revision {
            Some(r) if r == commit.expect_revision => {
                self.commit(commit.doc_id, &commit.label, commit.undoable, commit.content)
            }
            Some(r) => Err(format!(
                "文档版本过期：任务基于 r{}, 当前 r{}，已拒绝提交「{}」",
                commit.expect_revision, r, commit.label
            )),
            None => Err(format!("文档已关闭，拒绝提交「{}」", commit.label)),
        }
    }

    // ---- 撤销 / 重做 ----

    pub fn can_undo(&self, id: DocumentId) -> bool {
        self.get(id).map(|d| d.can_undo()).unwrap_or(false)
    }
    pub fn can_redo(&self, id: DocumentId) -> bool {
        self.get(id).map(|d| d.can_redo()).unwrap_or(false)
    }

    pub fn undo(&mut self, id: DocumentId) -> Result<String, String> {
        let doc = self.get_mut(id).ok_or_else(|| format!("文档不存在: {id:?}"))?;
        if !doc.can_undo() {
            return Err("没有可撤销的操作".to_string());
        }
        doc.history_pos -= 1;
        let entry = &doc.history[doc.history_pos];
        doc.content = entry.before.dup();
        doc.dirty = true;
        doc.revision += 1;
        Ok(entry.label.clone())
    }

    pub fn redo(&mut self, id: DocumentId) -> Result<String, String> {
        let doc = self.get_mut(id).ok_or_else(|| format!("文档不存在: {id:?}"))?;
        if !doc.can_redo() {
            return Err("没有可重做的操作".to_string());
        }
        let entry = &doc.history[doc.history_pos];
        let label = entry.label.clone();
        doc.content = entry.after.dup();
        doc.history_pos += 1;
        doc.dirty = true;
        doc.revision += 1;
        Ok(label)
    }

    // ---- 文件 IO ----

    /// 保存到已关联路径；无路径时返回错误（调用方应走“另存为”对话框）。
    pub fn save(&mut self, id: DocumentId) -> Result<PathBuf, String> {
        let (bytes, path) = {
            let doc = self.get(id).ok_or_else(|| format!("文档不存在: {id:?}"))?;
            let path = doc
                .path
                .clone()
                .ok_or_else(|| format!("文档「{}」尚未关联文件，请使用另存为", doc.title))?;
            let def = self
                .type_of(&doc.type_id)
                .ok_or_else(|| format!("未注册的文档类型: {}", doc.type_id))?;
            let bytes = (def.serialize)(doc.content.as_ref())?;
            (bytes, path)
        };
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
        let doc = self.get_mut(id).unwrap();
        doc.dirty = false;
        Ok(path)
    }

    pub fn save_as(&mut self, id: DocumentId, path: &Path) -> Result<PathBuf, String> {
        {
            let doc = self.get_mut(id).ok_or_else(|| format!("文档不存在: {id:?}"))?;
            doc.path = Some(path.to_path_buf());
            doc.title = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| doc.title.clone());
        }
        self.save(id)
    }

    /// 从文件打开：按扩展名匹配文档类型并反序列化，返回新文档 ID。
    pub fn open_file(&mut self, path: &Path) -> Result<DocumentId, String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let (type_id, deserialize) = {
            let def = self
                .find_type_by_extension(&ext)
                .ok_or_else(|| format!("没有登记 .{ext} 扩展名的文档类型"))?;
            (def.id.clone(), def.deserialize)
        };
        let bytes = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let content = deserialize(&bytes)?;
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".to_string());
        self.docs.push(Document {
            id,
            type_id,
            title,
            path: Some(path.to_path_buf()),
            dirty: false,
            revision: 0,
            content,
            history: Vec::new(),
            history_pos: 0,
        });
        Ok(id)
    }
}

impl Default for DocumentService {
    fn default() -> Self {
        Self::new()
    }
}
