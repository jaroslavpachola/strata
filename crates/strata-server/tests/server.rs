//! A server in a thread, a client in the test: the store's operations
//! over the socket, errors as the core's, and the event stream.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::json;
use strata_core::{
    Api, Entry, Error, Kind, Partition, PropertyDef, Query, Store, TypeDef, Values, VaultStatus,
};
use strata_server::{Client, Event, EventKind};
use tempfile::TempDir;
use tokio::sync::oneshot;

const PASS: &str = "server passphrase";

/// A running server; stops it when dropped.
struct Running {
    socket: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(dir: &Path, socket: &Path) -> Self {
        let store = Store::open(dir).unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let path = socket.to_path_buf();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(strata_server::serve(store, &path, async {
                let _ = stopped.await;
            }))
            .unwrap();
        });
        for _ in 0..200 {
            if std::os::unix::net::UnixStream::connect(socket).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Self {
            socket: socket.to_path_buf(),
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket).unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn setup() -> (TempDir, Running) {
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(&dir.path().join("store"), &dir.path().join("strata.sock"));
    (dir, running)
}

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

#[test]
fn the_store_over_the_socket() {
    let (dir, server) = setup();
    let mut client = server.client();
    assert_eq!(client.info().dir, dir.path().join("store"));
    assert_eq!(Api::dir(&client), Some(dir.path().join("store")));

    assert_eq!(client.vault_status().unwrap(), VaultStatus::Absent);
    client.vault_create(PASS).unwrap();
    let seeded = client.seed().unwrap();
    assert_eq!(seeded.added, ["Task", "PortfolioSnapshot"]);
    assert!(!client.seed_needs_vault().unwrap());

    let items = client
        .add_items(
            "Task",
            vec![
                values(json!({"title": "serve", "status": "doing"})),
                values(json!({"title": "stream", "status": "todo"})),
            ],
            "test",
        )
        .unwrap();
    let todo = client
        .query(&Query::new("Task").filter("status", "todo"))
        .unwrap();
    assert_eq!(todo.len(), 1);
    assert_eq!(todo[0].id(), items[1].id);

    let updated = client
        .update_item(items[0].id, values(json!({"status": "done"})), "other")
        .unwrap();
    assert_eq!(updated.modified_by, "other");
    client.relate(items[0].id, items[1].id, "then").unwrap();
    assert_eq!(client.relations(items[1].id).unwrap().len(), 1);
    client.unrelate(items[0].id, items[1].id, "then").unwrap();
    client.delete_item(items[1].id).unwrap();
    assert_eq!(client.query(&Query::new("Task")).unwrap().len(), 1);

    client
        .add_type(&TypeDef::new("Note").property(PropertyDef::new("text", Kind::Text)))
        .unwrap();
    client
        .add_property("Note", &PropertyDef::new("tag", Kind::Text))
        .unwrap();
    client.rename_property("Note", "tag", "label").unwrap();
    client.set_required("Note", "text", true).unwrap();
    client
        .set_choices("Note", "label", Some(vec!["a".into()]))
        .unwrap();
    client.set_description("Note", "jotted").unwrap();
    client.remove_property("Note", "label").unwrap();
    client.move_type("Note", Partition::Vault).unwrap();
    let note = client.get_type("Note").unwrap();
    assert_eq!(
        (
            note.partition,
            note.description.as_str(),
            note.properties.len()
        ),
        (Partition::Vault, "jotted", 1)
    );
    assert_eq!(client.list_types().unwrap().len(), 3);
}

#[test]
fn the_vault_is_unlocked_per_server_not_per_client() {
    let (_dir, server) = setup();
    let mut first = server.client();
    first.vault_create(PASS).unwrap();
    first.seed().unwrap();
    let snapshot = first
        .add_item(
            "PortfolioSnapshot",
            values(
                json!({"taken_at": "2026-09-19", "currency": "EUR", "total": 1, "positions": []}),
            ),
            "test",
        )
        .unwrap();

    // a second client sees it unlocked, and its lock locks it for both
    let mut second = server.client();
    assert_eq!(second.vault_status().unwrap(), VaultStatus::Unlocked);
    second.vault_lock().unwrap();
    assert_eq!(first.vault_status().unwrap(), VaultStatus::Locked);

    // the core's errors come back as themselves
    assert!(matches!(
        first.get_item(snapshot.id),
        Err(Error::VaultLocked)
    ));
    assert!(matches!(first.get(snapshot.id).unwrap(), Entry::Locked(_)));
    let err = first.vault_unlock("wrong").unwrap_err();
    assert_eq!(err.code(), "wrong_passphrase");
    let err = first.get_type("Nope").unwrap_err();
    assert_eq!(err.code(), "unknown_type");
    assert!(err.to_string().contains("Nope"));

    first.vault_unlock(PASS).unwrap();
    assert_eq!(
        second.get_item(snapshot.id).unwrap().values["currency"],
        "EUR"
    );
}

#[test]
fn a_subscriber_hears_what_changed() {
    let (_dir, server) = setup();
    let mut writer = server.client();
    writer.vault_create(PASS).unwrap();
    writer.seed().unwrap();

    // listen in a thread of its own, for Task only
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let socket = server.socket.clone();
    let listener = std::thread::spawn(move || {
        let client = Client::connect(&socket).unwrap();
        let events = client.events(Some("Task")).unwrap();
        ready_tx.send(()).unwrap();
        for event in events {
            if tx.send(event.unwrap()).is_err() {
                break;
            }
        }
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let task = writer
        .add_item(
            "Task",
            values(json!({"title": "t", "status": "todo"})),
            "test",
        )
        .unwrap();
    assert_eq!(next(), Event::items("Task"));

    // another type's change is not for this subscriber; the vault's is
    writer
        .add_item(
            "PortfolioSnapshot",
            values(
                json!({"taken_at": "2026-09-19", "currency": "EUR", "total": 1, "positions": []}),
            ),
            "test",
        )
        .unwrap();
    writer.vault_lock().unwrap();
    assert_eq!(next(), Event::vault());

    writer.delete_item(task.id).unwrap();
    assert_eq!(next(), Event::items("Task"));
    writer
        .add_property("Task", &PropertyDef::new("where", Kind::Text))
        .unwrap();
    let event = next();
    assert_eq!(
        (event.kind, event.type_name.as_deref()),
        (EventKind::Type, Some("Task"))
    );

    // stopping the server ends the stream
    drop(server);
    listener.join().unwrap();
}

#[test]
fn one_socket_one_server() {
    let (dir, server) = setup();
    let socket = dir.path().join("strata.sock");
    let store = Store::open(&dir.path().join("other")).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(strata_server::serve(store, &socket, async {}))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);

    // the first server's socket is gone when it stops, and a stale file
    // does not stop the next one
    drop(server);
    assert!(!socket.exists());
    std::fs::write(&socket, "").unwrap();
    let again = Running::start(&dir.path().join("store"), &socket);
    assert_eq!(again.client().vault_status().unwrap(), VaultStatus::Absent);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&socket).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn nobody_listening_is_an_error_at_connect() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::connect(&dir.path().join("none.sock"))
        .err()
        .unwrap();
    assert_eq!(err.code(), "server");
}
