//! `strata`: one-shot commands over the store, JSON in and out.
//!
//! Item bodies, patches and whole queries come in on stdin as JSON, so a
//! script (or an LLM writing one) never has to quote values into flags.
//! `--json` makes every answer JSON on stdout and every error JSON on
//! stderr. Exit codes: 0 ok, 1 error, 2 the vault is locked.
//!
//! With a strata-server running for the same store, every command goes
//! through it, so the two never fight over the files or the vault key.

mod config;
mod output;

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use strata_core::{
    Api, Kind, Partition, PropertyDef, Query, Sort, Store, TypeDef, Uuid, Values, VaultStatus,
    paths,
};
use strata_server::Client;

use crate::config::Config;
use crate::output::Out;

/// strata: a structured, local data store in two layers.
#[derive(Parser)]
#[command(name = "strata", version)]
struct Cli {
    /// The store's directory [default: $XDG_DATA_HOME/strata]
    #[arg(long, global = true, env = "STRATA_DIR")]
    dir: Option<PathBuf>,
    /// Answer in JSON, errors included
    #[arg(long, global = true)]
    json: bool,
    /// Who is writing: jarda, claude, a script's name [default: $USER]
    #[arg(long, global = true, env = "STRATA_AUTHOR")]
    author: Option<String>,
    /// Unlock the vault for this command, and lock it again after if it
    /// was locked; the passphrase comes from $STRATA_VAULT_PASSPHRASE,
    /// Barbero or a prompt
    #[arg(short, long, global = true)]
    unlock: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the store and its vault (unless --no-vault), and declare
    /// the seed types it lacks: Task, PortfolioSnapshot. The vault is
    /// locked when it is done
    Init {
        #[arg(long)]
        no_vault: bool,
    },
    /// Declare and inspect types
    #[command(subcommand)]
    Type(TypeCmd),
    /// Add, read, change and link items
    #[command(subcommand)]
    Item(ItemCmd),
    /// Items of one type. With no --type, the whole query is JSON on stdin:
    /// {"type", "filter", "sort": {"by", "descending"}, "limit", "offset"}
    Query {
        #[arg(short, long = "type")]
        type_name: Option<String>,
        /// PROPERTY=VALUE; VALUE is read as JSON if it parses, so 3 is a
        /// number and null is "no value"; quote it ('"3"') for a string
        #[arg(short = 'w', long = "where", value_name = "PROPERTY=VALUE")]
        filter: Vec<String>,
        /// created, modified, or a property
        #[arg(short, long)]
        sort: Option<String>,
        #[arg(long)]
        desc: bool,
        #[arg(long)]
        limit: Option<u64>,
        #[arg(long)]
        offset: Option<u64>,
    },
    /// The vault's state
    #[command(subcommand)]
    Vault(VaultCmd),
    /// Print changes as strata-server sees them, one per line, until it
    /// stops: items, type, or vault, and the type concerned
    Watch {
        #[arg(short, long = "type")]
        type_name: Option<String>,
    },
}

#[derive(Subcommand)]
enum TypeCmd {
    List,
    Show {
        name: String,
    },
    /// Declare a type. With no NAME, the whole definition is JSON on stdin:
    /// {"name", "partition", "description", "properties": [{"name", "kind", "required"}]}
    Add {
        name: Option<String>,
        /// NAME:KIND[!][=A|B|C]: ! for required, =A|B|C for the only
        /// values a text property takes. Kinds: text, number, date, bool,
        /// json, ref
        #[arg(short, long = "prop", value_name = "NAME:KIND[!]")]
        props: Vec<String>,
        /// Keep its items in the vault
        #[arg(long)]
        vault: bool,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Move a type and all its items to the other partition
    Move {
        name: String,
        #[arg(value_parser = parse_partition)]
        to: Partition,
    },
    /// Change a type's description
    Describe {
        name: String,
        description: String,
    },
    /// Change a type's properties
    Prop {
        #[arg(value_name = "TYPE")]
        type_name: String,
        #[command(subcommand)]
        change: PropCmd,
    },
}

#[derive(Subcommand)]
enum PropCmd {
    /// Add NAME:KIND[!][=A|B|C] at the end
    Add {
        spec: String,
    },
    /// Remove a property and every value of it
    Remove {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Require {
        name: String,
    },
    Optional {
        name: String,
    },
    /// Close a text property to A|B|C, or open it again with no CHOICES
    Choices {
        name: String,
        choices: Option<String>,
    },
}

#[derive(Subcommand)]
enum ItemCmd {
    /// Values on stdin: one object, or an array of them for several items
    Add {
        #[arg(value_name = "TYPE")]
        type_name: String,
    },
    Get {
        id: Uuid,
    },
    /// A patch on stdin: a key sets a property, null removes it
    Update {
        id: Uuid,
    },
    Delete {
        id: Uuid,
    },
    Relate {
        source: Uuid,
        target: Uuid,
        kind: String,
    },
    Unrelate {
        source: Uuid,
        target: Uuid,
        kind: String,
    },
    /// Every relation the item is an end of
    Relations {
        id: Uuid,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// absent, locked or unlocked
    Status,
    /// Unlock the server's vault until `vault lock` or the server stops.
    /// With no server there is nothing to keep it open: this checks the
    /// passphrase, and --unlock opens the vault per command
    Unlock,
    /// Lock the server's vault
    Lock,
}

fn main() -> ExitCode {
    // clap's own usage errors exit 2, which here means "locked"
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 1 } else { 0 });
        }
    };
    let out = Out { json: cli.json };
    match run(cli, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let locked = matches!(
                e.downcast_ref::<strata_core::Error>(),
                Some(strata_core::Error::VaultLocked)
            );
            out.error(&e, locked);
            ExitCode::from(if locked { 2 } else { 1 })
        }
    }
}

fn run(cli: Cli, out: &Out) -> anyhow::Result<()> {
    let config = Config::load()?;
    let dir = match cli.dir.clone() {
        Some(d) => d,
        None => paths::default_dir()?,
    };
    if let Command::Watch { type_name } = &cli.command {
        return watch(type_name.as_deref(), out);
    }
    let mut store = connect(&dir, matches!(cli.command, Command::Init { .. }))?;

    // --unlock is for this command: a vault that was locked is locked
    // again after it, even with a server that would keep it open
    let keep = matches!(
        cli.command,
        Command::Vault(VaultCmd::Unlock | VaultCmd::Lock)
    );
    let relock = cli.unlock && !keep && store.vault_status()? == VaultStatus::Locked;
    if (cli.unlock || matches!(cli.command, Command::Vault(VaultCmd::Unlock)))
        && store.vault_status()? == VaultStatus::Locked
    {
        store.vault_unlock(&config.passphrase(false)?)?;
    }
    let result = command(cli, store.as_mut(), &config, out);
    if relock {
        store.vault_lock()?;
    }
    result
}

/// The server, when one is running for this store; else the files.
/// `$STRATA_SOCKET` names a server to use whatever the directory, and it
/// must answer; the default socket is used only if its server holds the
/// same directory, so a script with its own `--dir` gets its own store.
fn connect(dir: &Path, creating: bool) -> anyhow::Result<Box<dyn Api>> {
    if let Some(socket) = std::env::var_os("STRATA_SOCKET").filter(|s| !s.is_empty()) {
        return Ok(Box::new(Client::connect(Path::new(&socket))?));
    }
    if let Ok(socket) = paths::default_socket()
        && let Ok(client) = Client::connect(&socket)
        && same_dir(&client.info().dir, dir)
    {
        return Ok(Box::new(client));
    }
    if !creating && !dir.join("open.db").exists() {
        bail!("no store at {}: run `strata init`", dir.display());
    }
    Ok(Box::new(Store::open(dir)?))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Print the server's events, one per line, until it stops.
fn watch(type_name: Option<&str>, out: &Out) -> anyhow::Result<()> {
    let socket = match std::env::var_os("STRATA_SOCKET").filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => paths::default_socket()?,
    };
    let client = Client::connect(&socket).context("watch needs a running strata-server")?;
    for event in client.events(type_name)? {
        let event = event?;
        out.value(&event, |v| {
            let kind = v["kind"].as_str().unwrap_or("?");
            match v["type"].as_str() {
                Some(t) => format!("{kind} {t}"),
                None => kind.to_string(),
            }
        })?;
        std::io::stdout().flush()?;
    }
    Ok(())
}

fn command(cli: Cli, store: &mut dyn Api, config: &Config, out: &Out) -> anyhow::Result<()> {
    let author = || -> anyhow::Result<String> {
        cli.author
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .filter(|a| !a.trim().is_empty())
            .context("who is writing? pass --author or set $STRATA_AUTHOR")
    };

    match cli.command {
        Command::Watch { .. } => unreachable!("handled in run"),

        Command::Init { no_vault } => {
            // a vault opened to create it or to seed it is locked again
            // after: init sets up, it does not leave a server unlocked
            let mut opened = false;
            if !no_vault {
                match store.vault_status()? {
                    VaultStatus::Absent => {
                        store.vault_create(&config.passphrase(true)?)?;
                        opened = true;
                    }
                    VaultStatus::Locked if store.seed_needs_vault()? => {
                        store.vault_unlock(&config.passphrase(false)?)?;
                        opened = true;
                    }
                    _ => {}
                }
            }
            let seeded = store.seed()?;
            if opened {
                store.vault_lock()?;
            }
            let vault = store.vault_status()?;
            let dir = store.dir().unwrap_or_default();
            out.value(
                &json!({"dir": dir, "vault": vault, "added": seeded.added, "skipped": seeded.skipped}),
                |v| {
                    let mut s = format!(
                        "{}  vault {}",
                        dir.display(),
                        v["vault"].as_str().unwrap_or("?")
                    );
                    for (label, key) in [("declared", "added"), ("skipped, vault locked", "skipped")] {
                        let names: Vec<_> = v[key]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect();
                        if !names.is_empty() {
                            s.push_str(&format!("\n{label}: {}", names.join(", ")));
                        }
                    }
                    s
                },
            )
        }

        Command::Type(cmd) => match cmd {
            TypeCmd::List => out.types(&store.list_types()?),
            TypeCmd::Show { name } => out.types(&[store.get_type(&name)?]),
            TypeCmd::Add {
                name,
                props,
                vault,
                description,
            } => {
                let def = match name {
                    None => {
                        if !props.is_empty() || vault || description.is_some() {
                            bail!("a type on stdin carries its own properties and partition");
                        }
                        serde_json::from_value(stdin_json()?).context("a type definition")?
                    }
                    Some(name) => {
                        let mut def =
                            TypeDef::new(name).description(description.unwrap_or_default());
                        if vault {
                            def = def.partition(Partition::Vault);
                        }
                        for spec in &props {
                            def = def.property(parse_prop(spec)?);
                        }
                        def
                    }
                };
                store.add_type(&def)?;
                out.types(&[store.get_type(&def.name)?])
            }
            TypeCmd::Move { name, to } => {
                store.move_type(&name, to)?;
                out.types(&[store.get_type(&name)?])
            }
            TypeCmd::Describe { name, description } => {
                store.set_description(&name, &description)?;
                out.types(&[store.get_type(&name)?])
            }
            TypeCmd::Prop { type_name, change } => {
                match change {
                    PropCmd::Add { spec } => store.add_property(&type_name, &parse_prop(&spec)?)?,
                    PropCmd::Remove { name } => store.remove_property(&type_name, &name)?,
                    PropCmd::Rename { from, to } => {
                        store.rename_property(&type_name, &from, &to)?
                    }
                    PropCmd::Require { name } => store.set_required(&type_name, &name, true)?,
                    PropCmd::Optional { name } => store.set_required(&type_name, &name, false)?,
                    PropCmd::Choices { name, choices } => store.set_choices(
                        &type_name,
                        &name,
                        choices.as_deref().map(parse_choices),
                    )?,
                }
                out.types(&[store.get_type(&type_name)?])
            }
        },

        Command::Item(cmd) => match cmd {
            ItemCmd::Add { type_name } => {
                let author = author()?;
                match stdin_json()? {
                    Value::Array(bodies) => {
                        let bodies = bodies
                            .into_iter()
                            .map(object)
                            .collect::<anyhow::Result<_>>()?;
                        out.items(&store.add_items(&type_name, bodies, &author)?)
                    }
                    body => out.item(&store.add_item(&type_name, object(body)?, &author)?),
                }
            }
            ItemCmd::Get { id } => out.item(&store.get_item(id)?),
            ItemCmd::Update { id } => {
                let patch = object(stdin_json()?)?;
                out.item(&store.update_item(id, patch, &author()?)?)
            }
            ItemCmd::Delete { id } => {
                store.delete_item(id)?;
                out.value(&json!({"deleted": id}), |_| format!("deleted {id}"))
            }
            ItemCmd::Relate {
                source,
                target,
                kind,
            } => {
                store.relate(source, target, &kind)?;
                out.relations(&store.relations(source)?)
            }
            ItemCmd::Unrelate {
                source,
                target,
                kind,
            } => {
                store.unrelate(source, target, &kind)?;
                out.relations(&store.relations(source)?)
            }
            ItemCmd::Relations { id } => out.relations(&store.relations(id)?),
        },

        Command::Query {
            type_name,
            filter,
            sort,
            desc,
            limit,
            offset,
        } => {
            let q = match type_name {
                None => {
                    if !filter.is_empty()
                        || sort.is_some()
                        || desc
                        || limit.is_some()
                        || offset.is_some()
                    {
                        bail!(
                            "a query on stdin carries its own filter, sort and page; or pass --type"
                        );
                    }
                    serde_json::from_value(stdin_json()?).context("a query")?
                }
                Some(type_name) => {
                    let mut q = Query::new(type_name);
                    for f in &filter {
                        let (property, value) = f
                            .split_once('=')
                            .with_context(|| format!("--where {f:?}: expected PROPERTY=VALUE"))?;
                        let value = serde_json::from_str(value)
                            .unwrap_or_else(|_| Value::String(value.to_string()));
                        q = q.filter(property, value);
                    }
                    if sort.is_some() || desc {
                        q.sort = Some(Sort {
                            by: sort.unwrap_or_else(|| "created".into()),
                            descending: desc,
                        });
                    }
                    q.limit = limit;
                    q.offset = offset;
                    q
                }
            };
            out.entries(&store.query(&q)?)
        }

        Command::Vault(cmd) => {
            if let VaultCmd::Lock = cmd {
                store.vault_lock()?;
            }
            let status = store.vault_status()?;
            out.value(&json!({"vault": status}), |v| {
                v["vault"].as_str().unwrap_or("?").to_string()
            })
        }
    }
}

/// NAME:KIND, a ! after the kind for required, and =A|B|C for a text
/// property's choices: `status:text!=todo|doing|done`.
fn parse_prop(spec: &str) -> anyhow::Result<PropertyDef> {
    let (name, rest) = spec
        .split_once(':')
        .with_context(|| format!("{spec:?}: expected NAME:KIND"))?;
    let (kind, choices) = match rest.split_once('=') {
        Some((kind, choices)) => (kind, Some(parse_choices(choices))),
        None => (rest, None),
    };
    let (kind, required) = match kind.strip_suffix('!') {
        Some(k) => (k, true),
        None => (kind, false),
    };
    let kind: Kind =
        serde_json::from_value(Value::String(kind.to_string())).with_context(|| {
            format!("{spec:?}: {kind:?} is not a kind: text, number, date, bool, json or ref")
        })?;
    let mut p = PropertyDef::new(name, kind);
    p.required = required;
    p.choices = choices;
    Ok(p)
}

fn parse_choices(s: &str) -> Vec<String> {
    s.split('|').map(str::to_string).collect()
}

fn parse_partition(s: &str) -> Result<Partition, String> {
    match s {
        "open" => Ok(Partition::Open),
        "vault" => Ok(Partition::Vault),
        _ => Err("open or vault".into()),
    }
}

fn stdin_json() -> anyhow::Result<Value> {
    if std::io::stdin().is_terminal() {
        bail!("expected JSON on stdin");
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    serde_json::from_str(&text).context("stdin is not JSON")
}

fn object(v: Value) -> anyhow::Result<Values> {
    match v {
        Value::Object(map) => Ok(map),
        other => bail!("expected a JSON object of values, got {other}"),
    }
}
