#![doc = include_str!("../docs/tasks_module.md")]

mod server;
mod task_index;
mod task_store;
mod types;

pub mod tools;

pub use server::{TasksMcp, TasksMcpArgs};
pub use task_index::TaskIndex;
pub use task_store::{TaskStore, TaskStoreError};
pub use tools::common::TaskSummary;
pub use tools::create::{TaskCreateInput, TaskCreateOutput, execute_task_create};
pub use tools::get::{TaskGetInput, TaskGetOutput, execute_task_get};
pub use tools::list::{TaskListInput, TaskListOutput, execute_task_list};
pub use tools::update::{TaskUpdateInput, TaskUpdateOutput, execute_task_update};
pub use types::{Task, TaskId, TaskStatus, TaskUpdate};
