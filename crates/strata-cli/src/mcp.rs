//! `strata mcp`: the store as an MCP server on stdio, for an LLM to
//! read and write.
//!
//! Blind to the vault whether it is locked or not: a vault type's items
//! come back as `{id, type, locked}` placeholders, and writes to a vault
//! type are refused, since the LLM should never see or set vault values.
//! What it needs from the vault goes through a script it writes and never
//! sees the output of.
//!
//! JSON-RPC 2.0, one message per line: `initialize`, `ping`, `tools/list`
//! and `tools/call`; notifications are read and not answered.

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use strata_core::{Api, Entry, Locked, Partition, Query, Sort, Uuid};

const INSTRUCTIONS: &str = "strata is the user's local store of structured data: typed items \
(tasks, and whatever types they declare) with properties. Start with strata_types to see \
what exists, then strata_query for one type's items. Writes go through strata_item_add, \
strata_item_update and strata_item_delete; only open-partition types accept writes — vault \
types are read-only placeholders, whether or not the vault is unlocked.";

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
        {
            "name": "strata_item_add",
            "description": "Create an item of the given open-partition type. Returns the \
                new item with its id. Vault types are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "description": "The type's name, e.g. Task"},
                    "values": {"type": "object", "description": "Property values, e.g. {\"title\": \"buy milk\", \"status\": \"todo\"}"},
                    "author": {"type": "string", "description": "Who is creating it; defaults to claude"},
                },
                "required": ["type", "values"],
            },
        },
        {
            "name": "strata_item_update",
            "description": "Update an item by id. values is a patch: present keys are set, \
                null removes a value. Returns the updated item. Vault items are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "The item's UUID"},
                    "values": {"type": "object", "description": "Patch: {\"status\": \"done\"} sets status, {\"note\": null} removes note"},
                    "author": {"type": "string", "description": "Who is writing; defaults to claude"},
                },
                "required": ["id", "values"],
            },
        },
        {
            "name": "strata_item_delete",
            "description": "Delete an item by id. Vault items are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "The item's UUID"},
                },
                "required": ["id"],
            },
        },
        {
            "name": "strata_relate",
            "description": "Create a named relation between two items, e.g. blocks, parent, \
                see_also. Idempotent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source item UUID"},
                    "target": {"type": "string", "description": "Target item UUID"},
                    "kind": {"type": "string", "description": "Relation kind, e.g. blocks"},
                },
                "required": ["source", "target", "kind"],
            },
        },
        {
            "name": "strata_unrelate",
            "description": "Remove a relation between two items.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source item UUID"},
                    "target": {"type": "string", "description": "Target item UUID"},
                    "kind": {"type": "string", "description": "Relation kind, e.g. blocks"},
                },
                "required": ["source", "target", "kind"],
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
            let id = parse_id(args)?;
            let entry = store.get(id).map_err(err)?;
            let vault = store
                .get_type(entry.type_name())
                .map(|d| d.partition == Partition::Vault)
                .map_err(err)?;
            Ok(json!(blind(entry, vault)))
        }
        "strata_item_add" => {
            let type_name = args["type"].as_str().ok_or("type is required")?;
            refuse_vault(store, type_name)?;
            let values = args["values"]
                .as_object()
                .ok_or("values is required")?
                .clone();
            let author = author(args);
            Ok(json!(
                store.add_item(type_name, values, &author).map_err(err)?
            ))
        }
        "strata_item_update" => {
            let id = parse_id(args)?;
            refuse_vault_item(store, id)?;
            let values = args["values"]
                .as_object()
                .ok_or("values is required")?
                .clone();
            let author = author(args);
            Ok(json!(store.update_item(id, values, &author).map_err(err)?))
        }
        "strata_item_delete" => {
            let id = parse_id(args)?;
            refuse_vault_item(store, id)?;
            store.delete_item(id).map_err(err)?;
            Ok(json!({"deleted": id.to_string()}))
        }
        "strata_relate" => {
            let source = parse_uuid(args, "source")?;
            let target = parse_uuid(args, "target")?;
            let kind = args["kind"].as_str().ok_or("kind is required")?;
            store.relate(source, target, kind).map_err(err)?;
            Ok(json!(store.relations(source).map_err(err)?))
        }
        "strata_unrelate" => {
            let source = parse_uuid(args, "source")?;
            let target = parse_uuid(args, "target")?;
            let kind = args["kind"].as_str().ok_or("kind is required")?;
            store.unrelate(source, target, kind).map_err(err)?;
            Ok(json!(store.relations(source).map_err(err)?))
        }
        _ => Err(format!("no tool {name:?}")),
    }
}

fn parse_id(args: &Value) -> Result<Uuid, String> {
    args["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s.trim_start_matches(crate::links::SCHEME)).ok())
        .ok_or_else(|| "id: expected a UUID".to_string())
}

fn parse_uuid(args: &Value, field: &str) -> Result<Uuid, String> {
    args[field]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("{field}: expected a UUID"))
}

fn author(args: &Value) -> String {
    args["author"]
        .as_str()
        .filter(|a| !a.is_empty())
        .unwrap_or("claude")
        .to_string()
}

fn refuse_vault(store: &dyn Api, type_name: &str) -> Result<(), String> {
    let def = store.get_type(type_name).map_err(|e| e.to_string())?;
    if def.partition == Partition::Vault {
        return Err(format!(
            "{type_name} is in the vault: MCP cannot write to vault types"
        ));
    }
    Ok(())
}

fn refuse_vault_item(store: &dyn Api, id: Uuid) -> Result<(), String> {
    let entry = store.get(id).map_err(|e| e.to_string())?;
    let def = store
        .get_type(entry.type_name())
        .map_err(|e| e.to_string())?;
    if def.partition == Partition::Vault {
        return Err(format!(
            "{} is in the vault: MCP cannot write to vault items",
            entry.type_name()
        ));
    }
    Ok(())
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
