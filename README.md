# strata

A structured, local data store for the desktop, in two layers: one open,
one encrypted. Typed items with relations instead of files in folders;
the desktop, scripts and an LLM read and write it through one API, and
the parts that should not sit in plaintext do not.

See [docs/PLAN.md](docs/PLAN.md) for the roadmap.

## Status

**M3 (0.3).** The core library and the `strata` CLI work: types, items,
relations and queries over two SQLite files, `open.db` in the clear and
`vault.db` under SQLCipher. While the vault is locked its items show only
as `{id, type, locked}` placeholders and every write to it is refused.
No server yet, so the vault is unlocked per command.

## Using it

    strata init                          # the store, and a vault (asks for a passphrase)
    strata type add Task -p title:text! -p status:text! -p due:date
    echo '{"title": "write M4", "status": "todo"}' | strata item add Task
    strata query -t Task -w status=todo --sort due
    strata -u type add Account --vault -p name:text! -p iban:text
    strata -u query -t Account           # without -u: placeholders only

Values, patches and whole queries go in on stdin as JSON; `--json` makes
every answer JSON, errors included. Exit codes: 0 ok, 1 error, 2 locked.

The vault passphrase comes from `$STRATA_VAULT_PASSPHRASE`, else from
Barbero when `~/.config/strata/config.toml` says so, else a prompt:

    cascade = "barbero"            # runs `barbero-cli get strata/vault`
    barbero_entry = "strata/vault"

Every write records an author: `--author`, `$STRATA_AUTHOR`, or `$USER`.

## Layout, once it exists

    crates/strata-core     schema, the two databases, vault state, query, export
    crates/strata-cli      one-shot commands, JSON in and out
    crates/strata-server   a Unix socket for long-lived clients
    crates/strata-tui      ratatui browser (M7)
    crates/strata-egui     egui widgets, standalone and as a Plocha pane (M8, M9)
