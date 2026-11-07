//! The Workbench module contract (design.md §8).
//!
//! Modules are statically linked when the product is assembled (Cargo crates) and register their
//! contributions in `init`; a module holds no runtime state itself — domain state lives in
//! extension services or document content.

use crate::ids::WorkbenchId;
use crate::registry::Registry;

pub trait Workbench: 'static {
    /// Stable module ID.
    fn id(&self) -> WorkbenchId;
    /// Register contributions. May panic — the core catches and isolates it during assembly
    /// (acceptance #19).
    fn init(&self, registry: &mut Registry);
}

/// Convenience: build a boxed assembly item for a type that implements Workbench.
pub fn boxed<W: Workbench>(wb: W) -> Box<dyn Workbench> {
    Box::new(wb)
}
