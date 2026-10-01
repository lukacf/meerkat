//! Observe actual lease acquisition polls without adding an await after Ready.
//! No observation supplies authority or changes the result of the owner future.
use crate::tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use meerkat_core::{AuthLoginLifecycleGuard, InputId};
use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::sync::{Mutex, OnceLock};
use std::task::Poll;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::meerkat_machine) enum Stage {
    Waiting,
    Acquired,
}

fn observers() -> &'static Mutex<HashMap<InputId, UnboundedSender<Stage>>> {
    static OBSERVERS: OnceLock<Mutex<HashMap<InputId, UnboundedSender<Stage>>>> = OnceLock::new();
    OBSERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::meerkat_machine) struct Observation {
    input: InputId,
    receiver: UnboundedReceiver<Stage>,
}

impl Observation {
    pub(in crate::meerkat_machine) fn install(input: &InputId) -> Self {
        let (sender, receiver) = unbounded_channel();
        let previous = observers()
            .lock()
            .expect("test observer lock")
            .insert(input.clone(), sender);
        assert!(previous.is_none(), "one observation per unique input");
        Self {
            input: input.clone(),
            receiver,
        }
    }

    pub(in crate::meerkat_machine) async fn next(&mut self) -> Option<Stage> {
        self.receiver.recv().await
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        observers()
            .lock()
            .expect("test observer cleanup")
            .remove(&self.input);
    }
}

fn record(input: &InputId, stage: Stage) {
    let sender = observers()
        .lock()
        .expect("test observer lock")
        .get(input)
        .cloned();
    if let Some(sender) = sender {
        let _ = sender.send(stage);
    }
}

pub(super) async fn observe(
    input: &InputId,
    acquisition: impl Future<Output = AuthLoginLifecycleGuard>,
) -> AuthLoginLifecycleGuard {
    let mut acquisition = std::pin::pin!(acquisition);
    let mut reported_wait = false;
    poll_fn(|cx| match acquisition.as_mut().poll(cx) {
        Poll::Pending => {
            if !reported_wait {
                reported_wait = true;
                record(input, Stage::Waiting);
            }
            Poll::Pending
        }
        Poll::Ready(guard) => {
            // Sending is synchronous. Do not suspend between owner acquisition
            // and returning the guard to the real admission implementation.
            record(input, Stage::Acquired);
            Poll::Ready(guard)
        }
    })
    .await
}
