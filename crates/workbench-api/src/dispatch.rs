//! Command dispatch for view elements.
//!
//! A view builds its content as an element tree, and that tree's event handlers
//! outlive the [`crate::view::ViewInstance::element`] call that produced them —
//! so they cannot borrow the platform host. The frontend publishes one
//! dispatcher for its runtime's lifetime (see [`publish_dispatcher`]) and
//! handlers go through [`dispatch`].

use std::cell::RefCell;
use std::rc::Rc;

use crate::command::{CommandArgs, CommandResult};

thread_local! {
    static DISPATCH: RefCell<Option<Rc<dyn Fn(&str, CommandArgs) -> CommandResult>>> =
        const { RefCell::new(None) };
}

/// Publish the dispatcher the view element handlers run through.
///
/// The frontend calls this once when it builds its runtime (a second publish
/// replaces the previous one — multi-window frontends publish per runtime
/// when each window's thread mounts its root view).
pub fn publish_dispatcher(f: Rc<dyn Fn(&str, CommandArgs) -> CommandResult>) {
    DISPATCH.with(|cell| *cell.borrow_mut() = Some(f));
}

/// Stop publishing the current dispatcher (runtime teardown).
pub fn clear_dispatcher() {
    DISPATCH.with(|cell| *cell.borrow_mut() = None);
}

/// Run a platform command on behalf of a view element's event handler.
pub fn dispatch(command: &str, args: CommandArgs) -> CommandResult {
    DISPATCH.with(|cell| {
        let cell = cell.borrow();
        match cell.as_ref() {
            Some(f) => f(command, args),
            None => CommandResult::failed("视图元素不在渲染上下文中"),
        }
    })
}
