//! The server: one [`Store`] behind a Unix socket.
//!
//! The vault is unlocked per server process, and every client shares
//! that state. The socket's file mode (0600, in `$XDG_RUNTIME_DIR`) is the
//! access control: one user, one machine, no tokens.

use std::convert::Infallible;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query as QueryParams, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strata_core::{Store, VaultStatus};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::BroadcastStream;

use crate::protocol::{ErrorBody, Event, Info, Request};

struct Shared {
    store: Mutex<Store>,
    dir: PathBuf,
    events: broadcast::Sender<Event>,
    /// Flips to true on shutdown, ending the event streams, which would
    /// otherwise hold the graceful shutdown open for ever.
    closing: watch::Receiver<bool>,
}

/// Serve `store` on `socket` until `shutdown` completes, then remove the
/// socket. A live server already on `socket` is an error; a stale socket
/// file is replaced.
pub async fn serve(
    store: Store,
    socket: &Path,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let listener = bind(socket)?;
    let (close, closing) = watch::channel(false);
    let shared = Arc::new(Shared {
        dir: store.dir().map(Path::to_path_buf).unwrap_or_default(),
        store: Mutex::new(store),
        events: broadcast::channel(256).0,
        closing,
    });
    let app = Router::new()
        .route("/v1/rpc", post(rpc))
        .route("/v1/events", get(events))
        .with_state(shared);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = close.send(true);
        })
        .await;
    let _ = std::fs::remove_file(socket);
    result
}

fn bind(socket: &Path) -> std::io::Result<UnixListener> {
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "a strata-server is already listening on {}",
                    socket.display()
                ),
            ));
        }
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

async fn rpc(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    let request: Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, "bad_request", e.to_string()),
    };
    let worker = shared.clone();
    // SQLite and SQLCipher's key derivation block: off the async threads
    let done = tokio::task::spawn_blocking(move || {
        let mut store = worker.store.lock().unwrap_or_else(|p| p.into_inner());
        handle(&mut store, &worker.dir, request)
    })
    .await;
    match done {
        Ok(Ok((value, events))) => {
            for event in events {
                // no subscribers is not an error
                let _ = shared.events.send(event);
            }
            axum::Json(value).into_response()
        }
        Ok(Err(e)) => error(status(e.code()), e.code(), e.to_string()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "server", e.to_string()),
    }
}

/// Run one request against the store: its answer, and what changed.
fn handle(
    store: &mut Store,
    dir: &Path,
    request: Request,
) -> strata_core::Result<(Value, Vec<Event>)> {
    fn json(v: impl Serialize) -> Value {
        serde_json::to_value(v).expect("store types serialize")
    }
    let nothing = |events: Vec<Event>| Ok((Value::Null, events));
    match request {
        Request::Info => Ok((
            json(Info {
                dir: dir.to_path_buf(),
                version: env!("CARGO_PKG_VERSION").into(),
            }),
            vec![],
        )),
        Request::VaultStatus => Ok((json(store.vault_status()), vec![])),
        Request::VaultCreate { passphrase } => {
            store.vault_create(&passphrase)?;
            nothing(vec![Event::vault()])
        }
        Request::VaultUnlock { passphrase } => {
            let was = store.vault_status();
            store.vault_unlock(&passphrase)?;
            nothing(if was == VaultStatus::Unlocked {
                vec![]
            } else {
                vec![Event::vault()]
            })
        }
        Request::VaultLock => {
            let was = store.vault_status();
            store.vault_lock()?;
            nothing(if was == VaultStatus::Unlocked {
                vec![Event::vault()]
            } else {
                vec![]
            })
        }
        Request::AddType { def } => {
            store.add_type(&def)?;
            nothing(vec![Event::type_changed(def.name)])
        }
        Request::ListTypes => Ok((json(store.list_types()?), vec![])),
        Request::GetType { name } => Ok((json(store.get_type(&name)?), vec![])),
        Request::SetDescription {
            type_name,
            description,
        } => {
            store.set_description(&type_name, &description)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::AddProperty {
            type_name,
            property,
        } => {
            store.add_property(&type_name, &property)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::RemoveProperty {
            type_name,
            property,
        } => {
            store.remove_property(&type_name, &property)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::RenameProperty {
            type_name,
            from,
            to,
        } => {
            store.rename_property(&type_name, &from, &to)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::SetRequired {
            type_name,
            property,
            required,
        } => {
            store.set_required(&type_name, &property, required)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::SetChoices {
            type_name,
            property,
            choices,
        } => {
            store.set_choices(&type_name, &property, choices)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::MoveType { type_name, to } => {
            store.move_type(&type_name, to)?;
            nothing(vec![Event::type_changed(type_name)])
        }
        Request::AddItems {
            type_name,
            bodies,
            author,
        } => {
            let items = store.add_items(&type_name, bodies, &author)?;
            Ok((json(items), vec![Event::items(type_name)]))
        }
        Request::GetItem { id } => Ok((json(store.get_item(id)?), vec![])),
        Request::Get { id } => Ok((json(store.get(id)?), vec![])),
        Request::UpdateItem { id, patch, author } => {
            let item = store.update_item(id, patch, &author)?;
            let event = Event::items(&item.type_name);
            Ok((json(item), vec![event]))
        }
        Request::DeleteItem { id } => {
            let type_name = store.get(id)?.type_name().to_string();
            store.delete_item(id)?;
            nothing(vec![Event::items(type_name)])
        }
        Request::Relate {
            source,
            target,
            kind,
        } => {
            let type_name = store.get(source)?.type_name().to_string();
            store.relate(source, target, &kind)?;
            nothing(vec![Event::items(type_name)])
        }
        Request::Unrelate {
            source,
            target,
            kind,
        } => {
            let type_name = store.get(source)?.type_name().to_string();
            store.unrelate(source, target, &kind)?;
            nothing(vec![Event::items(type_name)])
        }
        Request::Relations { id } => Ok((json(store.relations(id)?), vec![])),
        Request::Query { query } => Ok((json(store.query(&query)?), vec![])),
        Request::Seed => {
            let seeded = store.seed()?;
            let events = seeded.added.iter().map(Event::type_changed).collect();
            Ok((json(seeded), events))
        }
        Request::SeedNeedsVault => Ok((json(store.seed_needs_vault()?), vec![])),
    }
}

#[derive(Deserialize)]
struct EventsParams {
    #[serde(rename = "type")]
    type_name: Option<String>,
}

async fn events(
    State(shared): State<Arc<Shared>>,
    QueryParams(params): QueryParams<EventsParams>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut closing = shared.closing.clone();
    let stream = BroadcastStream::new(shared.events.subscribe())
        // a subscriber that lagged has missed events; it gets the next ones
        .filter_map(move |event| {
            let event = event
                .ok()
                .filter(|e| e.concerns(params.type_name.as_deref()));
            async move {
                event
                    .and_then(|e| SseEvent::default().json_data(e).ok())
                    .map(Ok)
            }
        })
        .take_until(async move {
            let _ = closing.wait_for(|c| *c).await;
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn status(code: &str) -> StatusCode {
    match code {
        "vault_locked" => StatusCode::LOCKED,
        "wrong_passphrase" => StatusCode::FORBIDDEN,
        "unknown_type" | "unknown_property" | "unknown_item" => StatusCode::NOT_FOUND,
        "no_vault" | "vault_exists" | "type_exists" | "property_exists" | "required_unmet"
        | "choices_unmet" => StatusCode::CONFLICT,
        "corrupt" | "sqlite" | "io" | "server" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

fn error(status: StatusCode, code: &str, message: String) -> Response {
    let body = ErrorBody {
        code: code.to_string(),
        error: message,
    };
    (status, axum::Json(body)).into_response()
}
