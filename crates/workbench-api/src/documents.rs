//! Generic document infrastructure (design.md §12.1/§12.2).
//!
//! The platform manages the document lifecycle (new/open/save/dirty flag/revision/undo history);
//! a Workbench registers the codec of its domain content through a [`DocumentTypeDef`], and the
//! content is stored type-erased as a [`DocumentContent`] — the platform stays out of the domain
//! data model.

use crate::ids::{DocumentId, DocumentTypeId};
use crate::storage::Location;

/// Domain document content. Through a blanket impl, every `Clone + Send + Sync + 'static` type
/// implements it automatically.
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

/// Document serialize function (supplied by the domain module; a failure returns a
/// user-readable error).
pub type DocumentSerializeFn = fn(&dyn DocumentContent) -> Result<Vec<u8>, String>;
/// Document deserialize function (supplied by the domain module).
pub type DocumentDeserializeFn = fn(&[u8]) -> Result<Box<dyn DocumentContent>, String>;

/// Delegated undo hooks (supplied by the domain module; `DocumentTypeDef::undo_hooks`).
///
/// When they are supplied the platform no longer keeps a snapshot history (full clones are too
/// heavy for parametric documents); undo/redo are forwarded straight to the command history the
/// domain content keeps itself. The `undoable` argument of `commit`/`edit` has no effect on
/// delegated types — the domain content manages undoability on its own.
#[derive(Clone, Copy)]
pub struct UndoHooks {
    /// Undo one step; returns the action label.
    pub undo: fn(&mut dyn DocumentContent) -> Result<String, String>,
    /// Redo one step; returns the action label.
    pub redo: fn(&mut dyn DocumentContent) -> Result<String, String>,
    pub can_undo: fn(&dyn DocumentContent) -> bool,
    pub can_redo: fn(&dyn DocumentContent) -> bool,
}

/// Document type registration: the codec comes from the domain module; the platform defines no
/// domain schema.
///
/// The view (`open_view`) is chosen from the document's content rather than being a fixed view
/// type: one document type can open different views depending on its content.
pub struct DocumentTypeDef {
    pub id: DocumentTypeId,
    pub title: String,
    /// Associated extensions (lowercase, no dot), matched to pick the type when opening a file.
    pub extensions: Vec<String>,
    pub create_default: fn() -> Box<dyn DocumentContent>,
    pub serialize: DocumentSerializeFn,
    pub deserialize: DocumentDeserializeFn,
    /// The view to open for this document's content. The platform calls it with
    /// the document's content, so one document type can open different views
    /// depending on what the document actually holds.
    pub open_view: Option<fn(&dyn DocumentContent) -> Option<crate::ids::ViewTypeId>>,
    /// Delegated undo hooks (None = the platform's snapshot undo, see [`UndoHooks`]).
    pub undo_hooks: Option<UndoHooks>,
    /// The plugin this document type was contributed by, if any.
    pub source: Option<String>,
}

impl DocumentTypeDef {
    /// Attribute this document type to a plugin, so disabling the plugin
    /// removes it.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// An undo history entry (snapshot style).
struct UndoEntry {
    label: String,
    before: Box<dyn DocumentContent>,
    after: Box<dyn DocumentContent>,
}

/// One open document.
pub struct Document {
    pub id: DocumentId,
    pub type_id: DocumentTypeId,
    pub title: String,
    /// The file the document was opened from or last saved to.
    pub location: Option<Location>,
    pub dirty: bool,
    /// Document revision: incremented on every commit (including undo/redo). Background tasks
    /// check their commits against it.
    pub revision: u64,
    pub content: Box<dyn DocumentContent>,
    history: Vec<UndoEntry>,
    /// Number of history entries applied (the top of the undo stack).
    history_pos: usize,
    /// A save is in flight ([`DocumentService::begin_save`]).
    saving: bool,
}

impl Document {
    pub fn can_undo(&self) -> bool {
        self.history_pos > 0
    }
    pub fn can_redo(&self) -> bool {
        self.history_pos < self.history.len()
    }
    /// The label of the latest history entry (for display in the UI).
    pub fn last_action_label(&self) -> Option<&str> {
        self.history.get(self.history_pos.wrapping_sub(1)).map(|e| e.label.as_str())
    }
    /// Maximum undo history depth.
    pub const MAX_HISTORY: usize = 100;
}

/// The generic document service. Owned by the UI thread (background tasks can only touch
/// documents through a DocCommit).
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

    // ---- Type registration ----

    pub fn register_type(&mut self, def: DocumentTypeDef) -> Result<(), String> {
        if self.types.iter().any(|t| t.id == def.id) {
            return Err(format!("Duplicate document type registration: {}", def.id));
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
        self.types.iter().find(|t| t.extensions.contains(&ext))
    }

    // ---- Queries ----

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

    /// The document's current revision (for commit policies and tests; None means the document
    /// does not exist / is closed).
    pub fn revision(&self, id: DocumentId) -> Option<u64> {
        self.get(id).map(|d| d.revision)
    }

    /// Typed read-only access to a document's content.
    pub fn read<T: DocumentContent, R>(
        &self,
        id: DocumentId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        self.get(id)?.content.as_any().downcast_ref::<T>().map(f)
    }

    // ---- Lifecycle ----

    pub fn create(&mut self, type_id: &DocumentTypeId, title: &str) -> Result<DocumentId, String> {
        let def = self
            .type_of(type_id)
            .ok_or_else(|| format!("Unregistered document type: {type_id}"))?;
        let content = (def.create_default)();
        Ok(self.push(type_id.clone(), title.to_string(), None, content))
    }

    fn push(
        &mut self,
        type_id: DocumentTypeId,
        title: String,
        location: Option<Location>,
        content: Box<dyn DocumentContent>,
    ) -> DocumentId {
        let id = DocumentId(self.next_id);
        self.next_id += 1;
        self.docs.push(Document {
            id,
            type_id,
            title,
            location,
            dirty: false,
            revision: 0,
            content,
            history: Vec::new(),
            history_pos: 0,
            saving: false,
        });
        id
    }

    /// Commit new document content at a transaction boundary.
    /// `undoable=false` explicitly declares an action that cannot be undone (design.md §12.2);
    /// delegated undo types (`undo_hooks`) take no snapshots, so the argument has no effect.
    pub fn commit(
        &mut self,
        id: DocumentId,
        label: &str,
        undoable: bool,
        new_content: Box<dyn DocumentContent>,
    ) -> Result<u64, String> {
        let delegated = self.undo_hooks_of(id).is_some();
        let doc = self.get_mut(id).ok_or_else(|| format!("No such document: {id:?}"))?;
        if !delegated {
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
                // An action that cannot be undone invalidates the existing history (so undo
                // cannot return to an inconsistent state).
                doc.history.clear();
                doc.history_pos = 0;
            }
        }
        doc.content = new_content;
        doc.dirty = true;
        doc.revision += 1;
        Ok(doc.revision)
    }

    /// Typed edit: snapshot before the change, modify in place with the domain module's logic,
    /// then record the history. Delegated undo types skip the before/after snapshots (full clones
    /// are too heavy for parametric documents).
    pub fn edit<T: DocumentContent>(
        &mut self,
        id: DocumentId,
        label: &str,
        undoable: bool,
        f: impl FnOnce(&mut T),
    ) -> Result<u64, String> {
        let delegated = self.undo_hooks_of(id).is_some();
        let type_ok = self
            .get(id)
            .ok_or_else(|| format!("No such document: {id:?}"))?
            .content
            .as_any()
            .downcast_ref::<T>()
            .is_some();
        if !type_ok {
            return Err(format!(
                "The document content type is not {}",
                std::any::type_name::<T>()
            ));
        }
        let before = (!delegated).then(|| self.get(id).unwrap().content.dup());
        {
            let doc = self.get_mut(id).unwrap();
            let typed = doc
                .content
                .as_any_mut()
                .downcast_mut::<T>()
                .expect("type checked above");
            f(typed);
        }
        if let Some(before) = before {
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
        }
        let doc = self.get_mut(id).unwrap();
        doc.dirty = true;
        doc.revision += 1;
        Ok(doc.revision)
    }

    /// Apply a background task's DocCommit (the host has checked the revision on the UI thread).
    pub fn apply_commit(&mut self, commit: crate::tasks::DocCommit) -> Result<u64, String> {
        let revision = self.get(commit.doc_id).map(|d| d.revision);
        match revision {
            Some(r) if r == commit.expect_revision => {
                self.commit(commit.doc_id, &commit.label, commit.undoable, commit.content)
            }
            Some(r) => Err(format!(
                "Stale document revision: the task was based on r{}, the document is at r{}; \
                 rejected the commit \"{}\"",
                commit.expect_revision, r, commit.label
            )),
            None => Err(format!(
                "The document is closed; rejected the commit \"{}\"",
                commit.label
            )),
        }
    }

    // ---- Undo / redo ----

    /// The document type's delegated undo hooks (None = the platform's snapshot undo).
    fn undo_hooks_of(&self, id: DocumentId) -> Option<UndoHooks> {
        let doc = self.get(id)?;
        self.type_of(&doc.type_id)?.undo_hooks
    }

    pub fn can_undo(&self, id: DocumentId) -> bool {
        if let Some(hooks) = self.undo_hooks_of(id) {
            return self
                .get(id)
                .map(|d| (hooks.can_undo)(d.content.as_ref()))
                .unwrap_or(false);
        }
        self.get(id).map(|d| d.can_undo()).unwrap_or(false)
    }
    pub fn can_redo(&self, id: DocumentId) -> bool {
        if let Some(hooks) = self.undo_hooks_of(id) {
            return self
                .get(id)
                .map(|d| (hooks.can_redo)(d.content.as_ref()))
                .unwrap_or(false);
        }
        self.get(id).map(|d| d.can_redo()).unwrap_or(false)
    }

    pub fn undo(&mut self, id: DocumentId) -> Result<String, String> {
        if let Some(hooks) = self.undo_hooks_of(id) {
            let doc = self.get_mut(id).ok_or_else(|| format!("No such document: {id:?}"))?;
            let label = (hooks.undo)(doc.content.as_mut())?;
            doc.dirty = true;
            doc.revision += 1;
            return Ok(label);
        }
        let doc = self.get_mut(id).ok_or_else(|| format!("No such document: {id:?}"))?;
        if !doc.can_undo() {
            return Err("Nothing to undo".to_string());
        }
        doc.history_pos -= 1;
        let entry = &doc.history[doc.history_pos];
        doc.content = entry.before.dup();
        doc.dirty = true;
        doc.revision += 1;
        Ok(entry.label.clone())
    }

    pub fn redo(&mut self, id: DocumentId) -> Result<String, String> {
        if let Some(hooks) = self.undo_hooks_of(id) {
            let doc = self.get_mut(id).ok_or_else(|| format!("No such document: {id:?}"))?;
            let label = (hooks.redo)(doc.content.as_mut())?;
            doc.dirty = true;
            doc.revision += 1;
            return Ok(label);
        }
        let doc = self.get_mut(id).ok_or_else(|| format!("No such document: {id:?}"))?;
        if !doc.can_redo() {
            return Err("Nothing to redo".to_string());
        }
        let entry = &doc.history[doc.history_pos];
        let label = entry.label.clone();
        doc.content = entry.after.dup();
        doc.history_pos += 1;
        doc.dirty = true;
        doc.revision += 1;
        Ok(label)
    }

    // ---- Open and save ----
    //
    // The service only encodes and decodes: reading and writing the file goes
    // through the storage provider that holds it, off the UI thread (see
    // `CommandCtx::read_file`). Opening is `type_for` → read → `open_bytes`;
    // saving is `begin_save` → write → `finish_save`.

    /// The document type registered for the extension of `file_name`.
    pub fn type_for(&self, file_name: &str) -> Result<DocumentTypeId, String> {
        let ext = crate::storage::split_file_name(file_name).1.unwrap_or_default();
        self.find_type_by_extension(ext)
            .map(|def| def.id.clone())
            .ok_or_else(|| format!("No document type is registered for the .{ext} extension"))
    }

    /// A document's content as its type's file format.
    pub fn encode(&self, id: DocumentId) -> Result<Vec<u8>, String> {
        let doc = self.get(id).ok_or_else(|| format!("No such document: {id:?}"))?;
        let def = self
            .type_of(&doc.type_id)
            .ok_or_else(|| format!("Unregistered document type: {}", doc.type_id))?;
        (def.serialize)(doc.content.as_ref())
    }

    /// Decode `bytes` as a document of `type_id` and add it, clean.
    /// `location` is the file they were read from; None leaves the document
    /// without a file, so its first save asks where to put it.
    pub fn open_bytes(
        &mut self,
        type_id: &DocumentTypeId,
        title: &str,
        location: Option<Location>,
        bytes: &[u8],
    ) -> Result<DocumentId, String> {
        let def = self
            .type_of(type_id)
            .ok_or_else(|| format!("Unregistered document type: {type_id}"))?;
        let content = (def.deserialize)(bytes)?;
        Ok(self.push(type_id.clone(), title.to_string(), location, content))
    }

    /// Encode a document to save it to `target`, or to its own file when
    /// `target` is None. Write the bytes to the returned save's location, then
    /// hand the save and the write's outcome to [`Self::finish_save`]. One
    /// save per document is in flight at a time, so writes can't land out of
    /// order.
    pub fn begin_save(
        &mut self,
        id: DocumentId,
        target: Option<Location>,
    ) -> Result<(PendingSave, Vec<u8>), String> {
        let doc = self.get(id).ok_or_else(|| format!("No such document: {id:?}"))?;
        if doc.saving {
            return Err(format!("Document \"{}\" is already being saved", doc.title));
        }
        let location = target
            .or_else(|| doc.location.clone())
            .ok_or_else(|| format!("Document \"{}\" has no file yet; use Save As", doc.title))?;
        let revision = doc.revision;
        let bytes = self.encode(id)?;
        let save = PendingSave {
            doc: id,
            location,
            revision,
        };
        self.get_mut(id).expect("checked above").saving = true;
        Ok((save, bytes))
    }

    /// Record how a save from [`Self::begin_save`] ended. On success the
    /// document lives at the save's location (a new location renames it after
    /// the file); it is clean unless it was edited while the write ran.
    pub fn finish_save(
        &mut self,
        save: PendingSave,
        written: Result<(), String>,
    ) -> Result<Location, String> {
        let doc = self
            .get_mut(save.doc)
            .ok_or_else(|| {
                format!(
                    "The document was closed; the result of saving to {} was discarded",
                    save.location
                )
            })?;
        doc.saving = false;
        written?;
        if doc.location.as_ref() != Some(&save.location) {
            let stem = save.location.file_stem();
            if !stem.is_empty() {
                doc.title = stem.to_string();
            }
            doc.location = Some(save.location.clone());
        }
        if doc.revision == save.revision {
            doc.dirty = false;
        }
        Ok(save.location)
    }
}

/// A save started by [`DocumentService::begin_save`].
#[derive(Debug)]
pub struct PendingSave {
    pub doc: DocumentId,
    /// Where the bytes go.
    pub location: Location,
    /// The revision that was encoded.
    revision: u64,
}

impl Default for DocumentService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Text(String);

    fn service() -> (DocumentService, DocumentId) {
        let mut docs = DocumentService::new();
        docs.register_type(DocumentTypeDef {
            id: DocumentTypeId::new("test.text"),
            title: "Text".into(),
            extensions: vec!["txt".into()],
            create_default: || Box::new(Text(String::new())),
            serialize: |c| Ok(c.as_any().downcast_ref::<Text>().unwrap().0.clone().into_bytes()),
            deserialize: |b| Ok(Box::new(Text(String::from_utf8_lossy(b).into_owned()))),
            open_view: None,
            undo_hooks: None,
            source: None,
        })
        .unwrap();
        let id = docs.create(&DocumentTypeId::new("test.text"), "Untitled").unwrap();
        docs.edit::<Text>(id, "type", true, |t| t.0 = "hello".into()).unwrap();
        (docs, id)
    }

    #[test]
    fn a_save_moves_the_document_to_its_file_and_cleans_it() {
        let (mut docs, id) = service();
        assert!(docs.begin_save(id, None).is_err(), "no file yet: save-as is needed");

        let target = Location::new("mem", "dir/note.txt");
        let (save, bytes) = docs.begin_save(id, Some(target.clone())).unwrap();
        assert_eq!(bytes, b"hello");
        assert!(docs.begin_save(id, None).is_err(), "one save in flight at a time");
        assert_eq!(docs.finish_save(save, Ok(())), Ok(target.clone()));

        let doc = docs.get(id).unwrap();
        assert_eq!(doc.title, "note");
        assert_eq!(doc.location.as_ref(), Some(&target));
        assert!(!doc.dirty);
    }

    #[test]
    fn an_edit_during_the_write_keeps_the_document_dirty() {
        let (mut docs, id) = service();
        let (save, _) = docs.begin_save(id, Some(Location::new("mem", "a.txt"))).unwrap();
        docs.edit::<Text>(id, "more", true, |t| t.0.push('!')).unwrap();
        docs.finish_save(save, Ok(())).unwrap();
        assert!(docs.get(id).unwrap().dirty);
    }

    #[test]
    fn a_failed_write_changes_nothing_but_allows_another_save() {
        let (mut docs, id) = service();
        let (save, _) = docs.begin_save(id, Some(Location::new("mem", "a.txt"))).unwrap();
        assert!(docs.finish_save(save, Err("disk full".into())).is_err());
        let doc = docs.get(id).unwrap();
        assert_eq!(doc.title, "Untitled");
        assert!(doc.location.is_none() && doc.dirty);
        assert!(docs.begin_save(id, Some(Location::new("mem", "a.txt"))).is_ok());
    }

    #[test]
    fn opening_picks_the_type_by_extension() {
        let (mut docs, _) = service();
        let location = Location::new("mem", "b.TXT");
        let type_id = docs.type_for(location.file_name()).unwrap();
        let id = docs
            .open_bytes(&type_id, location.file_stem(), Some(location.clone()), b"x")
            .unwrap();
        assert_eq!(docs.read::<Text, _>(id, |t| t.0.clone()).as_deref(), Some("x"));
        assert!(docs.type_for("c.bin").is_err());
    }
}
