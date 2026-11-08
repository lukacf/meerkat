//! A governed realm's terminal close (its session service's `try_shutdown`)
//! ends its custody of the grant owner its open restored, while a task of
//! the closed realm still holds that owner: the realm reopens at once in the
//! same process with its stock governance over the restored owner.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meerkat_store::RealmBackend;

use super::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
use super::*;

const REALM: &str = "governed";

async fn open(root: &Path, governance: RealmGovernance) -> crate::PersistenceBundle {
    open_governed_realm_persistence_with_provider(
        &crate::DiskStorageProvider,
        root,
        REALM,
        Some(RealmBackend::Sqlite),
        None,
        None,
        governance,
    )
    .await
    .expect("the realm opens")
    .1
}

/// The warnings the test records, as `tracing` formats them.
#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A stock realm opened from its existing grant store, closed through its
/// session service's terminal `try_shutdown` while a task of the closed
/// realm still holds the realm's grant owner, is reopened at once in the
/// same process. The reopen restores the grant owner and has the stock
/// governance, with no "did not restore" warning, rather than opening with
/// its governed work and configured MCP attachments unavailable.
#[tokio::test]
async fn a_closed_stock_realm_reopens_governed_while_its_grant_owner_is_still_held() {
    let sink = LogSink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(sink.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let tmp = tempfile::tempdir().unwrap();
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    let realm = meerkat_core::connection::RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(tmp.path(), REALM, &principals)
        .await
        .expect("governed, with its grant owner on disk");

    let first = StockGovernance::new(realm.clone(), principals.clone());
    let bundle = open(tmp.path(), RealmGovernance::Stock(Arc::clone(&first))).await;
    // Stands in for a task of the closed realm that still holds the owner.
    let held = first
        .composed_grant_owner_for_test()
        .expect("the open restored the realm's grant owner");
    let builder = crate::FactoryAgentBuilder::new(
        crate::AgentFactory::new(tmp.path().join("sessions")),
        crate::Config::default(),
    );
    let (service, machine) = crate::surface::build_runtime_backed_service(builder, 4, bundle)
        .expect("the realm's session service");
    service
        .try_shutdown()
        .await
        .expect("the realm's terminal close");
    drop(service);
    // The machine itself (and the realm's execution custody) is gone before
    // the reopen: only the held grant owner outlives the close.
    let closed_machine = Arc::downgrade(&machine);
    drop(machine);
    let deadline = Instant::now() + Duration::from_secs(30);
    while closed_machine.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "the closed realm's machine drops"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let second = StockGovernance::new(realm, principals);
    let reopened = open(tmp.path(), RealmGovernance::Stock(Arc::clone(&second))).await;
    assert!(reopened.stock_governance().is_some());
    assert!(
        second.composed_grant_owner_for_test().is_some(),
        "the reopened realm composes its stock governance over the restored owner"
    );
    let warnings = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    assert!(
        !warnings.contains("did not restore"),
        "the reopen restores the realm's grant owner: {warnings}"
    );
    drop(held);
}
