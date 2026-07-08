use crate::model::task::Task;

/// A task for building an inventory file for a given directory.
pub type InventoryBuilderTask = Task<(), String>;