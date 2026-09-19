# strata

A structured, local data store for the desktop, in two layers: one open,
one encrypted. Typed items with relations instead of files in folders;
the desktop, scripts and an LLM read and write it through one API, and
the parts that should not sit in plaintext do not.

See [docs/PLAN.md](docs/PLAN.md) for the roadmap.

## Status

**M5 (0.5).** The core library, the `strata` CLI and `strata-server`
work: types, items, relations and queries over two SQLite files,
`open.db` in the clear and `vault.db` under SQLCipher. While the vault is
locked its items show only as `{id, type, locked}` placeholders and every
write to it is refused. `strata init` seeds a Task type (open) and a
PortfolioSnapshot type (vault), and `scripts/portfolio-snapshot` fills
the latter from a stand-in provider.

## Using it

    strata init                          # the store, a vault, the seed types
    echo '{"title": "write M5", "status": "todo"}' | strata item add Task
    strata type add Bug -p title:text! -p 'state:text!=open|closed'   # ! required, =A|B choices
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

### The server

    strata-server &                      # $XDG_RUNTIME_DIR/strata.sock, mode 0600
    strata vault unlock                  # stays unlocked until `vault lock` or exit
    strata watch --type Task             # changes, one JSON line each with --json

With a server running for the same store, the CLI goes through it, so
the two never fight over the files or the vault key; `$STRATA_SOCKET`
points the CLI at a particular one. Without a server, `-u` unlocks the
vault for one command.

`scripts/portfolio-snapshot` takes a portfolio snapshot into the vault:
the API key comes from Barbero (`PORTFOLIO_KEY`, default `portfolio/api`)
and goes straight into the fetch command's stdin; the script prints only
the new snapshot's id. The fetch is a stand-in until a provider is wired
in: see the script's header for the contract.

## Layout, once it exists

    crates/strata-core     schema, the two databases, vault state, query, export
    crates/strata-cli      one-shot commands, JSON in and out
    crates/strata-server   a Unix socket for long-lived clients, and its client
    crates/strata-tui      ratatui browser (M7)
    crates/strata-egui     egui widgets, standalone and as a Plocha pane (M8, M9)
