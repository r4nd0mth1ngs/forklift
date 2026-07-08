use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::model::task::base_task_context::BaseTaskContext;
use crate::types::task::Task;

pub trait TaskContext<O: Send, E: Clone + Send> {
    /// Get the base task context.
    fn get_base_context(&self) -> Arc<BaseTaskContext<O, E>>;

    /// Send a task to the task queue.
    /// This task will be executed by one of the workers.
    ///
    /// # Arguments
    /// * `task` - The task to send.
    ///
    /// # Returns
    /// * `Ok(())`      - If the task was sent successfully.
    /// * `Err(String)` - If an error occurred while sending the task.
    fn send_task(&self, task: Task<O, E>) -> Result<(), String> {
        let base_context = self.get_base_context();
        base_context.task_counter.fetch_add(1, Ordering::SeqCst);

        base_context.task_sender.send(task).map_err(|e|
            format!("Error while sending task: {}", e)
        )
    }
}