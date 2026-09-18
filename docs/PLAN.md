# strata: plan

A structured, local, partly encrypted data store for the desktop. Data,
not files: typed items with relations, queried by whatever wants to show
them. The name is the design: two layers, one open and one locked.

## Why

The desktop's working state - tasks, contacts, bookmarks, project state,
a portfolio - lives today in files, in apps that each own their format,
or nowhere. Plocha wants to draw that state in panes and cannot draw what
has no store. SuperHub holds prose and is good at it; it should not
become a database. Barbero holds secrets and should hold nothing else.
strata is the layer between: structured, queryable, and honest about
which parts need a passphrase.

The design conversation is in SuperHub: `Ideas/infosys-draft.md` and
`Projects/infosys.md`. This file is the version that ships with the code.

## Where the pieces already are

- **Barbero** (`~/projects/barbero`): `barbero-core` opens an fpm2 vault,
  `barbero-cli get <TITLE>` prints one password. That is the whole
  contract strata needs for the passphrase cascade; nothing new on the
  Barbero side.
- **Plocha** (`~/projects/plocha`): `plocha-core` is the canvas with no
  egui in it, `plocha-egui` draws panes and already hosts rcmd as one.
  A strata pane goes in the same place rcmd's does, and the same rule
  applies: the store must be usable with no window open.
- **rcmd** (`~/projects/rcmd`): the ratatui and egui front ends over one
  `App` are the pattern for strata-tui and strata-egui. Edition 2024,
  workspace with `crates/`, a justfile with `check` = test + clippy +
  fmt, tags per phase, crates.io every few phases.
- **SuperHub**: the vault of notes, with its `author` field on every
  note. strata items carry the same provenance.

## Shape

    strata-cli      one-shot commands, JSON in and out, for scripts and Claude
    strata-server   long-lived, a Unix socket, for the desktop and the TUI
    strata-tui      ratatui browser over the store (later)
    strata-egui     egui widgets over the store, standalone and as a Plocha pane (later)
            |
    strata-core     the library: schema, two databases, vault state, query, export

Two SQLite files under `$XDG_DATA_HOME/strata/`:

    open.db     plain SQLite, always readable
    vault.db    SQLCipher, needs a passphrase, holds what must not sit in plaintext

Both carry the same meta-schema and the API shows them as one store. A
type is assigned to a partition; moving a type moves its items. When the
vault is locked its items surface as `Locked { id, type }` placeholders,
so an open task can point at a vault entity and the reference stays
visible while the target does not.

The meta-schema, editable at runtime:

    Type        name, partition, description
    Property    type, name, kind (text, number, date, bool, json, ref), required
    Item        id (UUIDv7), type, created, modified, author
    Value       item, property, value (JSON)
    Relation    from, to, kind

Every write records an author: `jarda`, `claude`, `gemini`, or a script's
name. Same discipline as SuperHub.

## Milestones

Each milestone ends with a tag. crates.io only every few, once a tag has
sat unchanged for a while.

### M0 - Bootstrap

- Workspace, `crates/strata-core` and `crates/strata-cli`, justfile, CI.
- `rusqlite` with `bundled-sqlcipher`. One test proves the point: write to
  an encrypted db, close, reopen with the wrong key and fail, reopen with
  the right one and read.
- SQLCipher derives the key from the passphrase itself (PBKDF2, its
  default parameters). No second KDF in v1.

*Gate: the SQLCipher round trip passes on this machine, clippy clean.*

### M1 - Meta-schema and the open store

- Migrations for the five tables, run on open, versioned.
- `Store::open(dir)` opens `open.db`; the vault is a later concern.
- Types: add, list, get, change properties. Items: add, get, update,
  delete. Values validated against the property kind.
- Query: by type, property equality, sort by a property or by
  created/modified, limit and offset. No expression language yet.
- Tests run against an in-memory database.

*Gate: a Task type can be declared, filled and queried from a test.*

### M2 - The vault

- `Store` grows a second connection, opened with a key. `vault_unlock`,
  `vault_lock`, `vault_status`.
- `partition: open | vault` on every type. Changing it copies the items
  and deletes the source inside one transaction across both databases;
  if the vault is locked the change is refused.
- Queries span both when unlocked. Locked, vault items come back as
  placeholders and writes to vault types return an error.
- Relations across partitions resolve to placeholders the same way.
- Wrong passphrase is an error the caller can show, not a panic.

*Gate: the same test suite passes with the Task type in either
partition, and a locked store never leaks a vault value.*

### M3 - The CLI

- `strata type add|list|show|move`, `strata item add|get|update|delete`,
  `strata query`, `strata vault unlock|lock|status`, `strata init`.
- `--json` on every command; item bodies on stdin as JSON so an
  LLM-written script can pipe into it without quoting hell.
- Passphrase, in order: `STRATA_VAULT_PASSPHRASE`, then `barbero-cli get
  strata/vault` when `cascade = barbero` is set in the config, then a
  prompt.
- Exit codes: 0 ok, 1 error, 2 locked.

*Gate: a shell script creates a type, adds ten items and queries them
back, in both partitions, driven only by the CLI.*

### M4 - Seed types and the first real script

- `strata init` declares, idempotently:
  - **Task** (open): title, status (todo | doing | done), due, project,
    note (a SuperHub vault path).
  - **PortfolioSnapshot** (vault): taken_at, currency, total, positions
    as JSON `[{symbol, quantity, price, value}]`.
- One script under `scripts/`: fetch a portfolio with a key held in
  Barbero, write a snapshot. This is the layered model end to end:
  Barbero for the secret, strata-vault for the result, the LLM that
  wrote the script never seeing either.

*Gate: a snapshot is in the vault, put there by the script, and `strata
query --type PortfolioSnapshot` shows it only after unlock.*

### M5 - The server

- `crates/strata-server`: axum over a Unix socket at
  `$XDG_RUNTIME_DIR/strata.sock`, the same operations as the CLI, JSON
  bodies.
- The vault is unlocked per server process; the socket's file mode is
  the access control. No tokens, no TLS, one user, one machine.
- The CLI learns to talk to a running server instead of opening the
  files, so the two never fight over the vault key.
- A subscription endpoint: long-poll or SSE, "items of type T changed",
  so a pane can redraw without polling.

*Gate: the CLI passes M3's script unchanged against a running server.*

### M6 - Export

- `strata export` writes each open type as a markdown table plus a JSON
  file into `SuperHub/References/strata/`, one file per type.
- Vault types export only into an `age`-encrypted archive, never
  plaintext, and only when unlocked.
- Import is the reverse, so the data survives the app and so a schema
  change can go through export, edit, import.

*Gate: export, wipe the data dir, import, and the query results match.*

### M7 - The TUI

- `crates/strata-tui`: ratatui, one binary `strata-tui`.
- A table per type, columns from the properties, sort by heading, a
  filter line at the bottom, enter to open an item in a form, `u` to
  unlock the vault.
- Talks to the server when one is running, opens the files otherwise.
- rcmd's conventions: keymap file, the same colour scheme, a `--config`.

*Gate: every M4 seed type can be browsed and edited without the CLI.*

### M8 - egui widgets

- `crates/strata-egui`: a library, not a binary at first. Widgets:
  `TableView`, `FormView`, `KanbanView`, each taking a query and drawing
  the result. A `StrataBrowser` that puts a type picker beside them.
- A thin `strata-desktop` binary wraps the browser in an eframe window
  for use before Plocha is ready.
- The widgets hold no store handle of their own; they take a `&Store` or
  a server client, so Plocha can hand them its own.

*Gate: the desktop binary shows and edits the same data the TUI does.*

### M9 - A pane in Plocha

- In `plocha-egui`, next to the rcmd pane: a `StrataPane` that draws one
  `strata-egui` view. A pane is a query plus a view, as the draft said.
- Plocha owns one server client and unlocks the vault once; every pane
  shares it.
- Pane state (which type, which query, which view) is itself a strata
  item of type `Pane` in the open partition, so the desktop's layout
  survives a restart through the same store it displays.

*Gate: two panes, a Task table and a PortfolioSnapshot form, live on the
canvas, and closing and reopening Plocha brings them back.*

### M10 - The SuperHub bridge

- A note links to an item as `strata://item/<uuid>`; an item's `note`
  property is a vault path. `strata links` reports both directions.
- The SuperHub MCP server gains `strata_query` and `strata_item` tools,
  or strata ships its own small MCP server. Decide when M5 exists.
- The daily note prefill reads open tasks from strata instead of
  scraping recent daily notes.

*Gate: `daily-prefill` lists strata tasks.*

### Later, in no order

- **Sync.** A change table in each database, applied to a second machine
  over anything that moves files; or cr-sqlite. Ids are already UUIDv7.
- **A query language.** When JSON filtering gets slow: typed value
  columns and a small expression grammar (`status = todo and due < today`).
- **Timeline and graph views** in strata-egui.
- **Attachments**: a blob table in the vault for the odd PDF a record
  needs, or a pointer into the filesystem. Probably the pointer.
- **Key file** as a second factor on the vault, if Barbero grows one.
- **Contacts** as a third seed type, with the question of whether phone
  and address earn the vault or contacts stay whole and open.

## Open questions

- Values as JSON in one column, or one table per kind? JSON first;
  revisit at the query language.
- Does strata-egui need its own binary, or does Plocha arrive fast
  enough that M8's `strata-desktop` is never built? Build it; a window
  that opens in a second is how the widgets get tested.
- Server first or files first for the TUI? Both, with the same trait
  behind them, because the CLI needs the files path anyway.

## Testing

- `strata-core`: in-memory databases, every operation, in both
  partitions, locked and unlocked.
- `strata-cli`: shell-level tests that pipe JSON in and assert on JSON
  out, run under `just check`.
- `strata-server`: the CLI test suite against a socket.
- `strata-tui` and `strata-egui`: ratatui `TestBackend` and egui's
  headless harness, the way rcmd tests both front ends.
