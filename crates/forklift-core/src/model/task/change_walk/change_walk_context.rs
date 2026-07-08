use std::sync::Arc;
use tokio::sync::Mutex;
use crate::model::task::base_task_context::BaseTaskContext;
use crate::traits::task_context::TaskContext;
use crate::util::stocktake_utils::Change;

/// The context for the parallel change-collection walks (the staged and unstaged halves
/// of a stocktake): one task per directory, all appending to the shared change list.
pub struct ChangeWalkContext {
    base_context: Arc<BaseTaskContext<(), String>>,

    /// The collected changes, in no particular order — the caller sorts after the walk.
    pub changes: Arc<Mutex<Vec<Change>>>,
}

impl ChangeWalkContext {
    /// Create a new change walk context.
    ///
    /// # Returns
    /// * `ChangeWalkContext` - The new context.
    pub fn new() -> Self {
        Self {
            base_context: Arc::new(BaseTaskContext::new()),
            changes: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TaskContext<(), String> for ChangeWalkContext {
    /// Get the base context.
    ///
    /// # Returns
    /// * `Arc<BaseTaskContext>` - The base context.
    fn get_base_context(&self) -> Arc<BaseTaskContext<(), String>> {
        Arc::clone(&self.base_context)
    }
}
