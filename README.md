# strata

A structured, local data store for the desktop, in two layers: one open,
one encrypted. Typed items with relations instead of files in folders;
the desktop, scripts and an LLM read and write it through one API, and
the parts that should not sit in plaintext do not.

See [docs/PLAN.md](docs/PLAN.md) for the roadmap.

## Status

**M0, not started.** Nothing builds yet; the plan is the whole repo.

## Layout, once it exists

    crates/strata-core     schema, the two databases, vault state, query, export
    crates/strata-cli      one-shot commands, JSON in and out
    crates/strata-server   a Unix socket for long-lived clients
    crates/strata-tui      ratatui browser (M7)
    crates/strata-egui     egui widgets, standalone and as a Plocha pane (M8, M9)
