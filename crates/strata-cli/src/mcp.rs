//! `strata mcp`: the store as an MCP server on stdio, for an LLM to read.
//!
//! Read-only, and blind to the vault whether it is locked or not: a vault
//! type's items come back as `{id, type, locked}` placeholders, and a
//! filter or a sort over a vault type's values is refused, since which
//! items match would say something about them. What the LLM needs from
//! the vault goes through a script it writes and never sees the output of.
//!
//! JSON-RPC 2.0, one message per line: `initialize`, `ping`, `tools/list`
//! and `tools/call`; notifications are read and not answered.

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use strata_core::{Api, Entry, Locked, Partition, Query, Sort, Uuid};

const INSTRUCTIONS: &str = "strata is the user's local store of structured data: typed items \
(tasks, and whatever types they declare) with properties. Start with strata_types to see \
what exists, then strata_query for one type's items. Read-only. Types in the vault partition \
show their items only as {id, type, locked} placeholders, whether or not the vault is \
unlocked; that is deliberate, not an error to work around.";

pub fn serve(store: &dyn Api) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(store, &msg),
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": format!("not JSON: {e}")}
            })),
        };
        if let Some(reply) = reply {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// One message in, its answer out; `None` for a notification.
pub fn handle(store: &dyn Api, msg: &Value) -> Option<Value> {
    let id = msg.get("id").filter(|v| !v.is_null()).cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "strata", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => Ok(call(store, &msg["params"])),
        _ => Err((-32601, format!("no method {method:?}"))),
    };
    let id = id?;
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "strata_types",
            "description": "Every type in the store: its name, partition (open or vault), \
                description and properties (name, kind, required, choices).",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "strata_query",
            "description": "Items of one type. filter is property -> value, all must match \
                (null matches a missing value); sort is created, modified or a property. \
                A vault type answers with placeholders only, and takes no filter or \
                property sort.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "description": "The type's name, e.g. Task"},
                    "filter": {"type": "object", "description": "e.g. {\"status\": \"todo\"}"},
                    "sort": {"type": "string"},
                    "descending": {"type": "boolean"},
                    "limit": {"type": "integer", "minimum": 1},
                    "offset": {"type": "integer", "minimum": 0},
                },
                "required": ["type"],
            },
        },
        {
            "name": "strata_item",
            "description": "One item by id, as strata://item/<id> links name it. A vault \
                item answers with a placeholder.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "string", "description": "The item's UUID"}},
                "required": ["id"],
            },
        },
    ])
}

fn call(store: &dyn Api, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = &params["arguments"];
    match tool(store, name, args) {
        Ok(v) => json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false}),
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

fn tool(store: &dyn Api, name: &str, args: &Value) -> Result<Value, String> {
    let err = |e: strata_core::Error| e.to_string();
    match name {
        "strata_types" => Ok(json!(store.list_types().map_err(err)?)),
        "strata_query" => {
            let type_name = args["type"].as_str().ok_or("type is required")?;
            let def = store.get_type(type_name).map_err(err)?;
            let mut q = Query::new(type_name);
            if let Some(filter) = args.get("filter").and_then(Value::as_object) {
                q.filter = filter.clone();
            }
            if let Some(by) = args["sort"].as_str() {
                q.sort = Some(Sort {
                    by: by.to_string(),
                    descending: args["descending"].as_bool().unwrap_or(false),
                });
            } else if args["descending"].as_bool() == Some(true) {
                q = q.descending();
            }
            q.limit = args["limit"].as_u64();
            q.offset = args["offset"].as_u64();
            let vault = def.partition == Partition::Vault;
            let by_value = q.sort.as_ref().is_some_and(|s| s.by != "created");
            if vault && (!q.filter.is_empty() || by_value) {
                return Err(format!(
                    "{type_name} is in the vault: its values stay out of MCP, so it takes no \
                     filter and no sort but created"
                ));
            }
            let entries = store.query(&q).map_err(err)?;
            Ok(json!(
                entries
                    .into_iter()
                    .map(|e| blind(e, vault))
                    .collect::<Vec<_>>()
            ))
        }
        "strata_item" => {
            let id: Uuid = args["id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s.trim_start_matches(crate::links::SCHEME)).ok())
                .ok_or("id: expected a UUID")?;
            let entry = store.get(id).map_err(err)?;
            let vault = store
                .get_type(entry.type_name())
                .map(|d| d.partition == Partition::Vault)
                .map_err(err)?;
            Ok(json!(blind(entry, vault)))
        }
        _ => Err(format!("no tool {name:?}")),
    }
}

/// A vault item as its placeholder, whatever the vault's state.
fn blind(entry: Entry, vault: bool) -> Entry {
    match entry {
        Entry::Item(item) if vault => Entry::Locked(Locked {
            id: item.id,
            type_name: item.type_name,
            locked: true,
        }),
        other => other,
    }
}
