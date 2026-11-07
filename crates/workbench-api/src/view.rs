//! The view contract (design.md §9): registration, lifecycle and presentation.

use crate::ids::ViewTypeId;
use crate::services::AppServices;

/// A view instance: the content unit shared by panels and central tabs.
///
/// Lifecycle: created by a factory → `element` every frame → dropped on close.
///
/// View content is a **frontend element tree**: during rendering the frontend calls
/// [`ViewInstance::element`], passing the platform host so the view can read live state, and the
/// view returns a type-erased element handle (`gpui::AnyElement` under the gpui frontend). Views
/// no longer paint pixels themselves — drawing is handled by the frontend's element system, and
/// input by the frontend's event system.
pub trait ViewInstance: 'static {
    /// View type ID (should match the registered ViewTypeId).
    fn type_id(&self) -> ViewTypeId;
    /// Title (shown on the central tab / panel header).
    fn title(&self) -> String;
    /// The frontend element tree of the view content.
    ///
    /// Returning `None` renders no content this frame. The concrete handle type is agreed between
    /// the frontend and its view extensions.
    fn element(&mut self, _host: &mut dyn AppServices) -> Option<Box<dyn std::any::Any>> {
        None
    }
    /// Whether background work is still running and the platform should keep scheduling frames.
    ///
    /// `element` is only called when the view is **repainted**, and gpui does not repaint while the
    /// UI is idle. Views with background tasks (such as a drawing's HLR generation) use this method
    /// to make the platform keep scheduling frames while the task is in flight; otherwise nobody
    /// picks up the task's result when it arrives on the channel — the view would stay stuck at
    /// "generating". Defaults to `false` (most views have no background work).
    fn wants_repaint(&self) -> bool {
        false
    }
}
