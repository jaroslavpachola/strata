# strata

A structured, local data store for the desktop, in two layers: one open,
one encrypted. Typed items with relations instead of files in folders;
the desktop, scripts and an LLM read and write it through one API, and
the parts that should not sit in plaintext do not.

See [docs/PLAN.md](docs/PLAN.md) for the roadmap.

## Status

**M1.** `strata-core` opens `open.db`, runs its migrations, and declares,
fills and queries types through a Rust API; SQLCipher is built in and
proven by a round-trip test, but the vault partition is M2. The `strata`
binary parses `--version` and nothing else yet.

## Layout, once it exists

    crates/strata-core     schema, the two databases, vault state, query, export
    crates/strata-cli      one-shot commands, JSON in and out
    crates/strata-server   a Unix socket for long-lived clients
    crates/strata-tui      ratatui browser (M7)
    crates/strata-egui     egui widgets, standalone and as a Plocha pane (M8, M9)
