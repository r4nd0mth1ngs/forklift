use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::model::task::base_task_context::BaseTaskContext;
use crate::model::tree_item::TreeItem;
use crate::traits::task_context::TaskContext;

/// The context for the parallel (bottom-up) tree build of a stack: one task per
/// inventoried directory, scheduled by dependency — a directory's task runs once all of
/// its child directories are built. The leaves are enqueued first; each completing task
/// decrements its parent's counter and enqueues the parent when it reaches zero.
pub struct TreeBuilderContext {
    base_context: Arc<BaseTaskContext<(), String>>,

    /// The built trees by directory key. A parent's task moves its children out of this
    /// map; after the walk only the root (key `""`) remains.
    pub built: Arc<Mutex<HashMap<String, TreeItem>>>,

    /// The number of unbuilt child directories per directory key. The task of a
    /// directory is enqueued exactly when its counter reaches zero.
    pub pending_children: Arc<Mutex<HashMap<String, usize>>>,
}

impl TreeBuilderContext {
    /// Create a new tree builder context.
    ///
    /// # Arguments
    /// * `pending_children` - The initial child counts per directory key.
    ///
    /// # Returns
    /// * `TreeBuilderContext` - The new context.
    pub fn new(pending_children: HashMap<String, usize>) -> Self {
        Self {
            base_context: Arc::new(BaseTaskContext::new()),
            built: Arc::new(Mutex::new(HashMap::new())),
            pending_children: Arc::new(Mutex::new(pending_children)),
        }
    }
}

impl TaskContext<(), String> for TreeBuilderContext {
    /// Get the base context.
    ///
    /// # Returns
    /// * `Arc<BaseTaskContext>` - The base context.
    fn get_base_context(&self) -> Arc<BaseTaskContext<(), String>> {
        Arc::clone(&self.base_context)
    }
}
