#!/usr/bin/env bash
# The M3 gate: a shell script creates a type, adds ten items and queries
# them back, in both partitions, driven only by the CLI.
#
# Run by tests/cli.rs with $STRATA pointing at the binary; runnable by
# hand too: STRATA=target/debug/strata crates/strata-cli/tests/gate.sh
set -euo pipefail

strata="${STRATA:?set STRATA to the strata binary}"
export STRATA_DIR="$(mktemp -d)"
trap 'rm -rf "$STRATA_DIR"' EXIT
export STRATA_CONFIG=/dev/null
export STRATA_AUTHOR=gate
export STRATA_VAULT_PASSPHRASE='gate passphrase'

fail() { echo "gate: $*" >&2; exit 1; }

"$strata" init --json >/dev/null

for partition in open vault; do
    type="Task_$partition"
    flags=()
    [ "$partition" = vault ] && flags=(--vault)

    "$strata" -u --json type add "$type" "${flags[@]}" \
        -p title:text! -p n:number! -p done:bool >/dev/null

    # ten items in one call, as a JSON array on stdin
    jq -n '[range(10) | {title: "task \(.)", n: ., done: (. % 2 == 0)}]' \
        | "$strata" -u --json item add "$type" >/dev/null

    all=$("$strata" -u --json query -t "$type" --sort n)
    [ "$(jq length <<<"$all")" = 10 ] || fail "$partition: expected 10 items"
    [ "$(jq -c '[.[].values.n]' <<<"$all")" = '[0,1,2,3,4,5,6,7,8,9]' ] \
        || fail "$partition: wrong order: $all"
    [ "$(jq -r '.[0].author' <<<"$all")" = gate ] || fail "$partition: author"

    done_desc=$("$strata" -u --json query -t "$type" -w done=true --sort n --desc --limit 3)
    [ "$(jq -c '[.[].values.title]' <<<"$done_desc")" = '["task 8","task 6","task 4"]' ] \
        || fail "$partition: filter/sort/limit: $done_desc"

    # the same query, whole, on stdin
    same=$(jq -n --arg t "$type" '{type: $t, filter: {done: true}, sort: {by: "n", descending: true}, limit: 3}' \
        | "$strata" -u --json query)
    [ "$same" = "$done_desc" ] || fail "$partition: stdin query differs"

    # change one, read it back
    id=$(jq -r '.[0].id' <<<"$all")
    echo '{"done": null, "title": "task zero"}' | "$strata" -u --json item update "$id" >/dev/null
    got=$("$strata" -u --json item get "$id")
    [ "$(jq -c '.values' <<<"$got")" = '{"n":0,"title":"task zero"}' ] \
        || fail "$partition: update: $got"
done

# without --unlock the vault answers with placeholders, and only those
locked=$("$strata" --json query -t Task_vault)
[ "$(jq length <<<"$locked")" = 10 ] || fail "locked: expected 10 placeholders"
[ "$(jq -c '[.[] | keys] | unique' <<<"$locked")" = '[["id","locked","type"]]' ] \
    || fail "locked: placeholders carry more than id and type: $locked"

# a filter on the locked vault is refused with exit code 2
set +e
"$strata" --json query -t Task_vault -w done=true >/dev/null 2>&1
code=$?
set -e
[ "$code" = 2 ] || fail "locked filter: exit $code, expected 2"

echo "gate: ok"
