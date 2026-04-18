use crate::Result;
use crate::protocol::client_events::ClientEvent;
use crate::protocol::server_events::ServerEvent;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Transport: Send {
    fn send(&mut self, event: ClientEvent) -> BoxFuture<'_, Result<()>>;
    fn next_event(&mut self) -> BoxFuture<'_, Result<Option<ServerEvent>>>;
}
