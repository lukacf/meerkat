//! Shell tool module for command execution
//!
//! This module provides shell command execution capabilities for Meerkat agents
//! using Nushell as the backend. It supports both synchronous and asynchronous
//! (background) execution.
//!
//! ## Types
//!
//! - [`JobId`] - Unique identifier for background jobs
//! - [`JobStatus`] - Status of a background job
//! - [`BackgroundJob`] - Full job information
//! - [`JobSummary`] - Lightweight job info for listing
//! - [`ShellConfig`] - Shell tool configuration
//! - [`ShellError`] - Shell-related errors
//! - [`ShellTool`] - Built-in tool for shell command execution
//! - [`ShellOutput`] - Output from shell command execution
//! - [`JobManager`] - Manager for background job execution
//! - [`ShellJobStatusTool`] - Tool to check background job status
//! - [`ShellJobsListTool`] - Tool to list all background jobs
//! - [`ShellJobCancelTool`] - Tool to cancel a background job
//! - [`ShellToolSet`] - Bundle of all shell tools with shared job manager

mod config;
mod job_cancel_tool;
mod job_manager;
mod job_status_tool;
mod jobs_list_tool;
mod tool;
mod tool_set;
mod types;

pub use config::{ShellConfig, ShellError};
pub use job_cancel_tool::ShellJobCancelTool;
pub use job_manager::JobManager;
pub use job_status_tool::ShellJobStatusTool;
pub use jobs_list_tool::ShellJobsListTool;
pub use tool::{ShellOutput, ShellTool};
pub use tool_set::ShellToolSet;
pub use types::{BackgroundJob, JobId, JobStatus, JobSummary};
