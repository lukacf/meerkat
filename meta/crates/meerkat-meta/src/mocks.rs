//! Test doubles and mock implementations for development and testing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockProcessError {
    LaunchFailed,
    Timeout,
}

#[derive(Debug, Clone)]
enum MockBehavior {
    FailThenSucceed { failures_before_success: u32 },
    NeverResponds,
}

#[derive(Debug, Clone)]
pub struct MockModuleProcess {
    behavior: MockBehavior,
    attempts: Arc<AtomicU32>,
}

impl MockModuleProcess {
    pub fn fail_then_succeed(failures_before_success: u32) -> Self {
        Self {
            behavior: MockBehavior::FailThenSucceed {
                failures_before_success,
            },
            attempts: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn never_responds() -> Self {
        Self {
            behavior: MockBehavior::NeverResponds,
            attempts: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn invoke_json_line_with_timeout(
        &self,
        timeout: Duration,
        success_line: &str,
    ) -> Result<String, MockProcessError> {
        match self.behavior {
            MockBehavior::FailThenSucceed {
                failures_before_success,
            } => {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
                if attempt < failures_before_success {
                    return Err(MockProcessError::LaunchFailed);
                }
                Ok(success_line.to_string())
            }
            MockBehavior::NeverResponds => {
                let started = Instant::now();
                while started.elapsed() < timeout {
                    std::thread::yield_now();
                }
                Err(MockProcessError::Timeout)
            }
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::SeqCst)
    }
}
