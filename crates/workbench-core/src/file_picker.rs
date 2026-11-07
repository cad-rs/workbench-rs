//! The in-app file picker's I/O. Its state is [`api::FilePicker`] in
//! `ws.file_picker` and the frontend draws it; moving around updates the
//! state and lists the folder now shown, off the UI thread. The runtime opens
//! the picker for a file dialog request, and
//! [`api::AppServices::finish_file_dialog`] closes it.

use workbench_api as api;
use api::{CommandArgs, CommandCtx, CommandDef, CommandResult, Registry};

/// The command the picker's "New Folder" prompt commits (`value`: the name).
pub const NEW_FOLDER: &str = "app.file_picker.new_folder";

/// List the folder the open picker shows, into the picker.
pub fn list(ctx: &mut CommandCtx) {
    let Some(dir) = ctx.host().ws.file_picker.as_ref().map(|picker| picker.dir()) else {
        return;
    };
    ctx.list_dir(
        dir.clone(),
        Box::new(move |app, listing| {
            if let Some(picker) = &mut app.host().ws.file_picker {
                picker.listed(&dir, listing);
            }
        }),
    );
}

/// Move the open picker (`step` is one of [`api::FilePicker::browse`],
/// `enter`, `up_to`), then list the folder it shows.
pub fn navigate(ctx: &mut CommandCtx, step: impl FnOnce(&mut api::FilePicker)) {
    let Some(picker) = &mut ctx.host().ws.file_picker else {
        return;
    };
    step(picker);
    list(ctx);
}

pub(crate) fn register(registry: &mut Registry) {
    registry.register_command(CommandDef::sync(
        NEW_FOLDER,
        "New Folder",
        |ctx: &mut CommandCtx, args: &CommandArgs| {
            let name = args.get("value").unwrap_or_default().trim().to_string();
            if name.is_empty() || name.contains(['/', '\\']) {
                return CommandResult::failed("A folder name can't be empty or contain / or \\");
            }
            let Some(dir) = ctx.host().ws.file_picker.as_ref().map(|picker| picker.dir()) else {
                return CommandResult::failed("No file picker is open");
            };
            ctx.create_dir(
                dir.child(&name),
                Box::new(|app, created| match created {
                    Ok(()) => list(&mut CommandCtx::new(app)),
                    Err(e) => app.host().log.error(e),
                }),
            );
            CommandResult::done()
        },
    ));
}
