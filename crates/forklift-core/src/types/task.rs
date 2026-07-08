use std::future::Future;
use std::pin::Pin;

/// A task. It is a future that returns a result.
///
/// # Arguments
/// * `OkType`  - The success result type of the task.
/// * `ErrType` - The error result type of the task.
pub type Task<OkType, ErrType> = Pin<Box<dyn Future<Output = Result<OkType, ErrType>> + Send>>;