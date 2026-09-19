//! A blocking client for strata-server: the same [`Api`] as a [`Store`],
//! over the socket. Each call is one HTTP/1.1 request on a fresh
//! connection, driven by a small runtime of the client's own.
//!
//! [`Store`]: strata_core::Store

use std::path::{Path, PathBuf};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HOST};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use strata_core::{
    Api, Entry, Error, Item, Partition, PropertyDef, Query, Relation, Result, Seeded, TypeDef,
    Uuid, Values, VaultStatus,
};
use tokio::net::UnixStream;
use tokio::runtime::Runtime;

use crate::protocol::{ErrorBody, Event, Info, Request};

pub struct Client {
    socket: PathBuf,
    rt: Runtime,
    info: Info,
}

impl Client {
    /// Connect to the server on `socket`, and ask it who it is: a socket
    /// with nobody listening is an error here, not at the first call.
    pub fn connect(socket: &Path) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut client = Self {
            socket: socket.to_path_buf(),
            rt,
            info: Info {
                dir: PathBuf::new(),
                version: String::new(),
            },
        };
        client.info = client.call(&Request::Info)?;
        Ok(client)
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn call<T: DeserializeOwned>(&self, request: &Request) -> Result<T> {
        let body = serde_json::to_vec(request).map_err(transport)?;
        self.rt.block_on(async {
            let response = self.send(hyper::Method::POST, "/v1/rpc", body).await?;
            let ok = response.status().is_success();
            let bytes = response
                .into_body()
                .collect()
                .await
                .map_err(transport)?
                .to_bytes();
            if ok {
                serde_json::from_slice(&bytes).map_err(transport)
            } else {
                let e: ErrorBody = serde_json::from_slice(&bytes).map_err(transport)?;
                Err(Error::from_remote(&e.code, &e.error))
            }
        })
    }

    async fn send(
        &self,
        method: hyper::Method,
        path: &str,
        body: Vec<u8>,
    ) -> Result<hyper::Response<Incoming>> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| Error::Server(format!("{}: {e}", self.socket.display())))?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(transport)?;
        // the connection runs beside the request, on this runtime
        tokio::spawn(connection);
        let request = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "strata")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(transport)?;
        sender.send_request(request).await.map_err(transport)
    }

    /// Changes as they happen, for one type or (with `None`) all. The
    /// subscription is live when this returns; iterating blocks until the
    /// next event, and ends when the server stops.
    pub fn events(&self, type_name: Option<&str>) -> Result<Events<'_>> {
        let path = match type_name {
            Some(t) => format!("/v1/events?type={t}"),
            None => "/v1/events".into(),
        };
        let response = self
            .rt
            .block_on(self.send(hyper::Method::GET, &path, Vec::new()))?;
        if !response.status().is_success() {
            return Err(Error::Server(format!("events: {}", response.status())));
        }
        Ok(Events {
            client: self,
            body: response.into_body(),
            buf: Vec::new(),
        })
    }
}

/// The server-sent events of [`Client::events`].
pub struct Events<'a> {
    client: &'a Client,
    body: Incoming,
    buf: Vec<u8>,
}

impl Iterator for Events<'_> {
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // an event is lines up to a blank line; keep-alives are
            // comments, and carry no data
            if let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
                let block: Vec<u8> = self.buf.drain(..end + 2).collect();
                let text = String::from_utf8_lossy(&block);
                let data: Vec<&str> = text
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect();
                if data.is_empty() {
                    continue;
                }
                return Some(serde_json::from_str(&data.join("\n")).map_err(transport));
            }
            match self.client.rt.block_on(self.body.frame()) {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.buf.extend_from_slice(&data);
                    }
                }
                Some(Err(e)) => return Some(Err(transport(e))),
                None => return None,
            }
        }
    }
}

fn transport(e: impl std::fmt::Display) -> Error {
    Error::Server(e.to_string())
}

impl Api for Client {
    fn dir(&self) -> Option<PathBuf> {
        Some(self.info.dir.clone())
    }
    fn vault_status(&self) -> Result<VaultStatus> {
        self.call(&Request::VaultStatus)
    }
    fn vault_create(&mut self, passphrase: &str) -> Result<()> {
        self.call(&Request::VaultCreate {
            passphrase: passphrase.into(),
        })
    }
    fn vault_unlock(&mut self, passphrase: &str) -> Result<()> {
        self.call(&Request::VaultUnlock {
            passphrase: passphrase.into(),
        })
    }
    fn vault_lock(&mut self) -> Result<()> {
        self.call(&Request::VaultLock)
    }
    fn add_type(&self, def: &TypeDef) -> Result<()> {
        self.call(&Request::AddType { def: def.clone() })
    }
    fn list_types(&self) -> Result<Vec<TypeDef>> {
        self.call(&Request::ListTypes)
    }
    fn get_type(&self, name: &str) -> Result<TypeDef> {
        self.call(&Request::GetType { name: name.into() })
    }
    fn set_description(&self, type_name: &str, description: &str) -> Result<()> {
        self.call(&Request::SetDescription {
            type_name: type_name.into(),
            description: description.into(),
        })
    }
    fn add_property(&self, type_name: &str, property: &PropertyDef) -> Result<()> {
        self.call(&Request::AddProperty {
            type_name: type_name.into(),
            property: property.clone(),
        })
    }
    fn remove_property(&self, type_name: &str, property: &str) -> Result<()> {
        self.call(&Request::RemoveProperty {
            type_name: type_name.into(),
            property: property.into(),
        })
    }
    fn rename_property(&self, type_name: &str, from: &str, to: &str) -> Result<()> {
        self.call(&Request::RenameProperty {
            type_name: type_name.into(),
            from: from.into(),
            to: to.into(),
        })
    }
    fn set_required(&self, type_name: &str, property: &str, required: bool) -> Result<()> {
        self.call(&Request::SetRequired {
            type_name: type_name.into(),
            property: property.into(),
            required,
        })
    }
    fn set_choices(
        &self,
        type_name: &str,
        property: &str,
        choices: Option<Vec<String>>,
    ) -> Result<()> {
        self.call(&Request::SetChoices {
            type_name: type_name.into(),
            property: property.into(),
            choices,
        })
    }
    fn move_type(&self, type_name: &str, to: Partition) -> Result<()> {
        self.call(&Request::MoveType {
            type_name: type_name.into(),
            to,
        })
    }
    fn add_items(&self, type_name: &str, bodies: Vec<Values>, author: &str) -> Result<Vec<Item>> {
        self.call(&Request::AddItems {
            type_name: type_name.into(),
            bodies,
            author: author.into(),
        })
    }
    fn get_item(&self, id: Uuid) -> Result<Item> {
        self.call(&Request::GetItem { id })
    }
    fn get(&self, id: Uuid) -> Result<Entry> {
        self.call(&Request::Get { id })
    }
    fn update_item(&self, id: Uuid, patch: Values, author: &str) -> Result<Item> {
        self.call(&Request::UpdateItem {
            id,
            patch,
            author: author.into(),
        })
    }
    fn delete_item(&self, id: Uuid) -> Result<()> {
        self.call(&Request::DeleteItem { id })
    }
    fn relate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        self.call(&Request::Relate {
            source,
            target,
            kind: kind.into(),
        })
    }
    fn unrelate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        self.call(&Request::Unrelate {
            source,
            target,
            kind: kind.into(),
        })
    }
    fn relations(&self, id: Uuid) -> Result<Vec<Relation>> {
        self.call(&Request::Relations { id })
    }
    fn query(&self, q: &Query) -> Result<Vec<Entry>> {
        self.call(&Request::Query { query: q.clone() })
    }
    fn seed(&self) -> Result<Seeded> {
        self.call(&Request::Seed)
    }
    fn seed_needs_vault(&self) -> Result<bool> {
        self.call(&Request::SeedNeedsVault)
    }
}
