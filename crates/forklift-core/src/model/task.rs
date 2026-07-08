pub mod inventory_builder;
pub mod base_task_context;
pub mod change_walk;
pub mod tree_builder;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::task::JoinSet;
use crate::model::task::base_task_context::BaseTaskContext;
use crate::traits::task_context::TaskContext;
use crate::types::task::Task;

/// A task executor. It executes tasks in parallel using multiple workers.
///
/// # Arguments
/// * `OkType`      - The success result type of the tasks.
/// * `ErrType`     - The error result type of the tasks.
/// * `ContextType` - The type of the task context.
pub struct TaskExecutor<OkType: Send + Clone, ErrType: Clone + Send, ContextType>
where
    ContextType: TaskContext<OkType, ErrType>
{
    context: Arc<ContextType>,
    _marker: std::marker::PhantomData<(OkType, ErrType)>,
}

impl<O, E, C> TaskExecutor<O, E, C>
where
    O: Send + Clone + 'static,
    E: Clone + Send + 'static,
    C: TaskContext<O, E>
{
    /// Create a new task executor.
    /// Note that creating a task executor does not start the workers.
    ///
    /// # Arguments
    /// * `context` - The task context.
    ///
    /// # Returns
    /// * `TaskExecutor` - The new task executor.
    pub fn new(context: Arc<C>) -> Self {
        Self {
            context,
            _marker: std::marker::PhantomData,
        }
    }

    /// Execute the given task and all tasks that are enqueued by the task in parallel,
    /// using multiple workers.
    ///
    /// The number of workers is equal to the number of logical CPUs.
    /// Since the tasks are executed in parallel, it is advised to store the results in the context.
    ///
    /// Note that calling this method will start the workers, so it is advised not to
    /// run multiple task executors at the same time (it should be safe, but affects performance).
    ///
    /// # Arguments
    /// * `task` - The task to execute.
    ///
    /// # Returns
    /// * `Ok(())`    - If the task was executed successfully.
    /// * `Err(None)` - If an error occurred while executing the task.
    pub async fn execute(&self, task: Task<O, E>) -> Result<(), Option<E>> {
        let num_workers = num_cpus::get();
        let base_context = self.context.get_base_context();
        let mut worker_join_set = JoinSet::new();

        // Send the task to the task queue. This task may enqueue more tasks.
        if self.context.send_task(task).is_err() {
            return Err(None);
        }

        // Start worker task.
        for _ in 0..num_workers {
            worker_join_set.spawn(worker(Arc::clone(&base_context)));
        }

        // Wait for all workers to finish (or an error to occur).
        while let Some(join_result) = worker_join_set.join_next().await {
            match join_result {
                // A worker died without reporting an error (i.e. it panicked). This must be
                // treated as a failure, otherwise the caller would mistake the aborted build
                // for a successful one.
                Err(_) => {
                    base_context.error_occurred.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(Err(_)) => break,
                Ok(Ok(is_finished)) if is_finished => break,
                _ => {}
            }
        }

        // Make sure to abort workers that are still waiting for tasks,
        // as there are no more tasks to be executed.
        worker_join_set.abort_all();

        // Check if an error occurred. If so, return the error.
        if base_context.error_occurred.load(Ordering::SeqCst) {
            let error = base_context.error_value.lock().await;

            return Err(error.as_ref().cloned());
        }

        Ok(())
    }
}

/// A worker. It receives tasks from the task queue in the context and executes them.
///
/// # Arguments
/// * `context` - The base task context.
///
/// # Returns
/// * `Ok(true)`  - If there are no more tasks to be executed. All workers should be stopped.
/// * `Ok(false)` - If there are still tasks to be executed.
/// * `Err(())`   - If an error occurred while executing a task. All workers should be stopped.
async fn worker<O: Send, E: Clone + Send>(context: Arc<BaseTaskContext<O, E>>) -> Result<bool, ()> {
    loop {
        if context.error_occurred.load(Ordering::SeqCst) {
            return Err(());
        }

        if context.task_counter.load(Ordering::SeqCst) == 0 {
            return Ok(true);
        }

        // Receive a task from the queue
        let task_result = context.task_receiver.recv_async().await;

        match task_result {
            Ok(task) => {
                // Execute the task
                if let Err(e) = task.await {
                    // If an error occurs, store the error value first, and only then set the
                    // error flag: readers of the flag must be able to rely on the value being
                    // present once the flag is set.
                    {
                        let mut error_value = context.error_value.lock().await;
                        *error_value = Some(e);
                    }

                    context.error_occurred.store(true, Ordering::SeqCst);

                    return Err(())
                } else {
                    // Decrement the task counter
                    let remaining = context.task_counter.fetch_sub(1, Ordering::SeqCst);

                    // There are no more tasks to be executed.
                    // We let the main process know by returning true.
                    // `fetch_sub` returns the previous value, so 1 means this was the
                    // last task (and `remaining - 1` would underflow the usize at 0).
                    if remaining <= 1 {
                        return Ok(true);
                    }
                }
            }
            // All senders are disconnected, so there will be no more tasks to execute.
            Err(_) => return Ok(true),
        }
    }
}