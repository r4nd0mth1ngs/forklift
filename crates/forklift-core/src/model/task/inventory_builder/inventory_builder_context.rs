use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::model::task::base_task_context::BaseTaskContext;
use crate::traits::task_context::TaskContext;

/// The context for the inventory builder task.
pub struct InventoryBuilderContext {
    base_context: Arc<BaseTaskContext<(), String>>,
    /// The paths of the new inventory files.
    pub new_inventory_paths: Arc<Mutex<BTreeSet<String>>>,

    /// The paths of existing inventory files.
    /// When comparing the working directory, the path of the given inventory file should be removed.
    /// Remaining paths are considered dirty  (their corresponding directories have been removed).
    /// These inventories should be removed.
    pub dirty_inventory_paths: Arc<Mutex<BTreeSet<String>>>,
}

impl InventoryBuilderContext {
    /// Create a new inventory builder context.
    ///
    /// # Returns
    /// * `InventoryBuilderContext` - The new inventory builder context.
    pub fn new() -> Self {
        Self {
            base_context: Arc::new(BaseTaskContext::new()),
            new_inventory_paths: Arc::new(Mutex::new(BTreeSet::new())),
            dirty_inventory_paths: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }
}

impl TaskContext<(), String> for InventoryBuilderContext {
    /// Get the base context.
    ///
    /// # Returns
    /// * `Arc<BaseTaskContext>` - The base context.
    fn get_base_context(&self) -> Arc<BaseTaskContext<(), String>> {
        Arc::clone(&self.base_context)
    }
}