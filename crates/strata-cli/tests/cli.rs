//! The CLI as a script sees it: JSON on stdin, JSON on stdout, and the
//! exit code.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_strata");
const PASS: &str = "cli passphrase";

/// A store directory and the environment to drive it with.
struct Env {
    dir: TempDir,
    config: Option<String>,
    passphrase: Option<&'static str>,
}

impl Env {
    /// An initialised store with a vault, the passphrase in the env.
    fn new() -> Self {
        let env = Self {
            dir: tempfile::tempdir().unwrap(),
            config: None,
            passphrase: Some(PASS),
        };
        env.ok(&["init"], None);
        env
    }

    fn run(&self, args: &[&str], stdin: Option<Value>) -> Output {
        let config = self.dir.path().join("config.toml");
        std::fs::write(&config, self.config.as_deref().unwrap_or("")).unwrap();
        let mut cmd = Command::new(BIN);
        cmd.arg("--json")
            .args(args)
            .env("STRATA_DIR", self.dir.path().join("store"))
            .env("STRATA_CONFIG", &config)
            .env("STRATA_AUTHOR", "test")
            .env_remove("STRATA_VAULT_PASSPHRASE")
            // never the user's own server
            .env_remove("STRATA_SOCKET")
            .env("XDG_RUNTIME_DIR", self.dir.path())
            .env_remove("SUPERHUB_VAULT_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(p) = self.passphrase {
            cmd.env("STRATA_VAULT_PASSPHRASE", p);
        }
        let mut child = cmd.spawn().unwrap();
        let mut pipe = child.stdin.take().unwrap();
        if let Some(body) = stdin {
            pipe.write_all(body.to_string().as_bytes()).unwrap();
        }
        drop(pipe);
        child.wait_with_output().unwrap()
    }

    /// Run, expect exit 0, and parse stdout.
    fn ok(&self, args: &[&str], stdin: Option<Value>) -> Value {
        let out = self.run(args, stdin);
        assert!(
            out.status.success(),
            "strata {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Run, expect exit `code`, and parse the JSON error on stderr.
    fn fails(&self, code: i32, args: &[&str], stdin: Option<Value>) -> Value {
        let out = self.run(args, stdin);
        assert_eq!(
            out.status.code(),
            Some(code),
            "strata {args:?}: stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.is_empty());
        serde_json::from_slice(&out.stderr).unwrap()
    }
}

fn with_account(env: &Env) -> String {
    env.ok(
        &[
            "-u",
            "type",
            "add",
            "Account",
            "--vault",
            "-p",
            "name:text!",
            "-p",
            "number:text",
        ],
        None,
    );
    let item = env.ok(
        &["-u", "item", "add", "Account"],
        Some(json!({"name": "savings", "number": "CZ65"})),
    );
    item["id"].as_str().unwrap().to_string()
}

/// The M3 gate, as a shell script.
#[test]
fn gate_script() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate.sh");
    let out = Command::new("bash")
        .arg(script)
        .env("STRATA", BIN)
        .env_remove("STRATA_SOCKET")
        .env("XDG_RUNTIME_DIR", "/nonexistent")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn exit_codes_say_what_went_wrong() {
    let env = Env::new();
    let id = with_account(&env);

    let err = env.fails(2, &["item", "get", &id], None);
    assert_eq!(err["locked"], true);
    env.fails(2, &["item", "add", "Account"], Some(json!({"name": "x"})));
    env.fails(2, &["type", "move", "Account", "open"], None);

    let err = env.fails(
        1,
        &["item", "get", "01a0b8c6-77cd-7760-948f-6aa17a012b2b"],
        None,
    );
    assert_eq!(err["locked"], false);
    env.fails(1, &["type", "show", "Nope"], None);
    // a usage error is 1 too, not clap's 2
    let out = env.run(&["no-such-command"], None);
    assert_eq!(out.status.code(), Some(1));

    // a query is answered, with placeholders
    let got = env.ok(&["query", "-t", "Account"], None);
    assert_eq!(got, json!([{"id": id, "type": "Account", "locked": true}]));
}

#[test]
fn a_wrong_passphrase_is_an_error_not_a_lock() {
    let mut env = Env::new();
    with_account(&env);
    env.passphrase = Some("wrong");
    let err = env.fails(1, &["-u", "query", "-t", "Account"], None);
    assert!(err["error"].as_str().unwrap().contains("wrong passphrase"));
    env.fails(1, &["vault", "unlock"], None);
}

#[test]
fn with_no_passphrase_anywhere_it_says_where_to_put_one() {
    let mut env = Env::new();
    env.passphrase = None;
    let err = env.fails(1, &["-u", "query", "-t", "Account"], None);
    assert!(
        err["error"]
            .as_str()
            .unwrap()
            .contains("STRATA_VAULT_PASSPHRASE")
    );
}

#[test]
fn barbero_supplies_the_passphrase_when_configured() {
    let mut env = Env::new();
    let id = with_account(&env);

    // a stand-in for barbero-cli: `get <entry>` prints one password
    let fake = env.dir.path().join("fake-barbero");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\n[ \"$1 $2\" = 'get strata/vault' ] || exit 3\necho '{PASS}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    env.passphrase = None;
    env.config = Some(format!(
        "cascade = \"barbero\"\nbarbero_command = \"{}\"\n",
        fake.display()
    ));
    let got = env.ok(&["-u", "item", "get", &id], None);
    assert_eq!(got["values"]["number"], "CZ65");

    // the environment still comes first
    env.passphrase = Some("wrong");
    env.fails(1, &["-u", "item", "get", &id], None);

    // and a Barbero that fails is an error, not a prompt
    env.passphrase = None;
    env.config = Some(format!(
        "cascade = \"barbero\"\nbarbero_command = \"{}\"\nbarbero_entry = \"other\"\n",
        fake.display()
    ));
    let err = env.fails(1, &["-u", "item", "get", &id], None);
    assert!(err["error"].as_str().unwrap().contains("get other failed"));
}

#[test]
fn a_bad_config_is_an_error() {
    let mut env = Env::new();
    env.config = Some("cascade = \"carrier pigeon\"\n".into());
    env.fails(1, &["vault", "status"], None);
}

#[test]
fn vault_status_and_init_are_idempotent() {
    let env = Env::new();
    assert_eq!(env.ok(&["vault", "status"], None)["vault"], "locked");
    assert_eq!(
        env.ok(&["-u", "vault", "status"], None)["vault"],
        "unlocked"
    );
    assert_eq!(env.ok(&["vault", "unlock"], None)["vault"], "unlocked");
    assert_eq!(env.ok(&["init"], None)["vault"], "locked");

    let bare = Env {
        dir: tempfile::tempdir().unwrap(),
        config: None,
        passphrase: None,
    };
    bare.fails(1, &["vault", "status"], None);
    assert_eq!(bare.ok(&["init", "--no-vault"], None)["vault"], "absent");
}

#[test]
fn a_batch_add_is_all_or_nothing() {
    let env = Env::new();
    let err = env.fails(
        1,
        &["item", "add", "Task"],
        Some(json!([{"title": "one", "status": "todo"}, {"title": "two", "status": "later"}])),
    );
    assert!(err["error"].as_str().unwrap().contains("status"));
    assert_eq!(env.ok(&["query", "-t", "Task"], None), json!([]));
}

#[test]
fn types_come_from_flags_or_stdin() {
    let env = Env::new();
    let from_stdin = env.ok(
        &["type", "add"],
        Some(json!({
            "name": "Bookmark",
            "description": "a link",
            "properties": [{"name": "url", "kind": "text", "required": true}]
        })),
    );
    let def = &from_stdin[0];
    assert_eq!(def["partition"], "open");
    assert_eq!(def["properties"][0]["required"], true);

    let changed = env.ok(&["type", "prop", "Bookmark", "add", "tags:json"], None);
    assert_eq!(
        changed[0]["properties"][1],
        json!({"name": "tags", "kind": "json", "required": false})
    );
    env.ok(
        &["type", "prop", "Bookmark", "rename", "tags", "labels"],
        None,
    );
    env.ok(&["type", "prop", "Bookmark", "require", "labels"], None);
    env.ok(&["type", "prop", "Bookmark", "optional", "labels"], None);
    env.ok(&["type", "prop", "Bookmark", "remove", "labels"], None);
    env.ok(&["type", "describe", "Bookmark", "worth keeping"], None);
    let shown = env.ok(&["type", "show", "Bookmark"], None);
    assert_eq!(shown[0]["description"], "worth keeping");
    assert_eq!(shown[0]["properties"].as_array().unwrap().len(), 1);

    env.fails(1, &["type", "add", "Bad", "-p", "x:colour"], None);
    env.fails(
        1,
        &["type", "add", "-p", "x:text"],
        Some(json!({"name": "X"})),
    );
    // Bookmark and the two seed types
    assert_eq!(env.ok(&["type", "list"], None).as_array().unwrap().len(), 3);
}

#[test]
fn where_values_read_as_json_when_they_parse() {
    let env = Env::new();
    env.ok(
        &["type", "add", "Thing", "-p", "label:text", "-p", "n:number"],
        None,
    );
    env.ok(
        &["item", "add", "Thing"],
        Some(json!([{"label": "3", "n": 3}, {"label": "plain"}])),
    );
    let by_number = env.ok(&["query", "-t", "Thing", "-w", "n=3"], None);
    assert_eq!(by_number.as_array().unwrap().len(), 1);
    let by_quoted = env.ok(&["query", "-t", "Thing", "-w", "label=\"3\""], None);
    assert_eq!(by_quoted.as_array().unwrap().len(), 1);
    let by_text = env.ok(&["query", "-t", "Thing", "-w", "label=plain"], None);
    assert_eq!(by_text[0]["values"]["label"], "plain");
    let missing = env.ok(&["query", "-t", "Thing", "-w", "n=null"], None);
    assert_eq!(missing[0]["values"]["label"], "plain");
}

#[test]
fn items_update_delete_and_relate() {
    let env = Env::new();
    let account = with_account(&env);
    let task = env.ok(
        &["item", "add", "Task"],
        Some(json!({"title": "check", "status": "todo"})),
    );
    let task = task["id"].as_str().unwrap();

    let rels = env.ok(&["item", "relate", task, &account, "about"], None);
    assert_eq!(
        rels,
        json!([{"source": task, "target": account, "kind": "about"}])
    );
    assert_eq!(
        env.ok(&["item", "relations", &account], None)
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let updated = env.ok(
        &["--author", "claude", "item", "update", task],
        Some(json!({"title": "check it"})),
    );
    assert_eq!(updated["modified_by"], "claude");
    assert_eq!(updated["author"], "test");

    env.ok(&["item", "unrelate", task, &account, "about"], None);
    assert_eq!(env.ok(&["item", "relations", task], None), json!([]));
    assert_eq!(
        env.ok(&["item", "delete", task], None),
        json!({"deleted": task})
    );
    env.fails(1, &["item", "get", task], None);
    env.fails(1, &["item", "update", &account], Some(json!([1])));
}

#[test]
fn init_declares_the_seed_types_once() {
    let bare = Env {
        dir: tempfile::tempdir().unwrap(),
        config: None,
        passphrase: Some(PASS),
    };
    let first = bare.ok(&["init", "--no-vault"], None);
    assert_eq!(first["added"], json!(["Task"]));
    assert_eq!(first["skipped"], json!(["PortfolioSnapshot"]));

    // the second run creates the vault and finishes the job
    let second = bare.ok(&["init"], None);
    assert_eq!(second["added"], json!(["PortfolioSnapshot"]));
    let third = bare.ok(&["init"], None);
    assert_eq!(third["added"], json!([]));
    assert_eq!(third["vault"], "locked", "nothing to seed, so no unlock");

    let task = bare.ok(&["type", "show", "Task"], None);
    assert_eq!(
        task[0]["properties"][1],
        json!({"name": "status", "kind": "text", "required": true, "choices": ["todo", "doing", "done"]})
    );
    let snapshot = bare.ok(&["type", "show", "PortfolioSnapshot"], None);
    assert_eq!(snapshot[0]["partition"], "vault");
}

#[test]
fn choices_come_from_the_spec_and_the_prop_command() {
    let env = Env::new();
    let def = env.ok(
        &[
            "type",
            "add",
            "Bug",
            "-p",
            "state:text!=open|closed",
            "-p",
            "title:text",
        ],
        None,
    );
    assert_eq!(
        def[0]["properties"][0]["choices"],
        json!(["open", "closed"])
    );
    assert_eq!(def[0]["properties"][0]["required"], true);
    env.fails(
        1,
        &["item", "add", "Bug"],
        Some(json!({"state": "wontfix"})),
    );

    env.ok(
        &[
            "type",
            "prop",
            "Bug",
            "choices",
            "state",
            "open|closed|wontfix",
        ],
        None,
    );
    env.ok(&["item", "add", "Bug"], Some(json!({"state": "wontfix"})));
    env.fails(
        1,
        &["type", "prop", "Bug", "choices", "state", "open"],
        None,
    );
    let opened = env.ok(&["type", "prop", "Bug", "choices", "state"], None);
    assert!(opened[0]["properties"][0].get("choices").is_none());
}

/// The M4 gate: a snapshot is in the vault, put there by the script, and
/// `strata query --type PortfolioSnapshot` shows it only after unlock.
#[test]
fn the_portfolio_script_writes_a_snapshot_into_the_vault() {
    let env = Env::new();
    let barbero = env.dir.path().join("fake-barbero");
    std::fs::write(
        &barbero,
        "#!/bin/sh\n[ \"$1 $2\" = 'get portfolio/api' ] || exit 3\necho 'the-api-key'\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &barbero,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let config = env.dir.path().join("config.toml");
    std::fs::write(&config, "").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/portfolio-snapshot");
    let out = Command::new(script)
        .env("STRATA", BIN)
        .env("BARBERO", &barbero)
        .env("STRATA_DIR", env.dir.path().join("store"))
        .env("STRATA_CONFIG", &config)
        .env("STRATA_VAULT_PASSPHRASE", PASS)
        .env_remove("STRATA_SOCKET")
        .env("XDG_RUNTIME_DIR", env.dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // it says which snapshot, and nothing of what is in it
    let id = stdout.trim().strip_prefix("snapshot ").unwrap().to_string();
    for secret in ["the-api-key", "VWCE", "EUR", "5107"] {
        assert!(!stdout.contains(secret), "{secret} in the script's output");
    }

    let locked = env.ok(&["query", "--type", "PortfolioSnapshot"], None);
    assert_eq!(
        locked,
        json!([{"id": id, "type": "PortfolioSnapshot", "locked": true}])
    );

    let open = env.ok(&["-u", "query", "--type", "PortfolioSnapshot"], None);
    let snapshot = &open[0];
    assert_eq!(snapshot["id"], id.as_str());
    assert_eq!(snapshot["author"], "portfolio-snapshot");
    assert_eq!(snapshot["values"]["currency"], "EUR");
    assert_eq!(snapshot["values"]["total"], 5107.91);
    let positions = snapshot["values"]["positions"].as_array().unwrap();
    assert_eq!(positions.len(), 3);
    assert_eq!(positions[2]["value"], 2003.57);
}

/// A strata-server in a thread, on `<dir>/strata.sock` for `<dir>/store`:
/// where an [`Env`] on the same directory finds it by default.
struct Server {
    socket: std::path::PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(dir: &Path) -> Self {
        let socket = dir.join("strata.sock");
        let store = strata_core::Store::open(&dir.join("store")).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let path = socket.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(strata_server::serve(store, &path, async {
                let _ = stopped.await;
            }))
            .unwrap();
        });
        for _ in 0..200 {
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Self {
            socket,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// The M5 gate: M3's script, unchanged, against a running server.
#[test]
fn gate_script_against_a_server() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(dir.path());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate.sh");
    let out = Command::new("bash")
        .arg(script)
        .env("STRATA", BIN)
        .env("STRATA_SOCKET", &server.socket)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // it all went to the server's store, not the script's own directory
    let store = strata_core::Store::open(&dir.path().join("store")).unwrap();
    drop(server);
    assert_eq!(store.list_types().unwrap().len(), 4);
}

#[test]
fn a_server_keeps_the_vault_open_until_locked() {
    let mut env = Env {
        dir: tempfile::tempdir().unwrap(),
        config: None,
        passphrase: Some(PASS),
    };
    let _server = Server::start(env.dir.path());
    // found by the default socket, since it serves the same directory
    assert_eq!(env.ok(&["init"], None)["vault"], "locked");
    let id = with_account(&env);

    // -u unlocks for one command and locks again after
    env.fails(2, &["item", "get", &id], None);

    // vault unlock stays, across commands and without a passphrase
    assert_eq!(env.ok(&["vault", "unlock"], None)["vault"], "unlocked");
    env.passphrase = None;
    let got = env.ok(&["item", "get", &id], None);
    assert_eq!(got["values"]["number"], "CZ65");
    // -u on an unlocked vault leaves it unlocked
    env.ok(&["-u", "vault", "status"], None);
    assert_eq!(env.ok(&["vault", "status"], None)["vault"], "unlocked");

    assert_eq!(env.ok(&["vault", "lock"], None)["vault"], "locked");
    env.fails(2, &["item", "get", &id], None);
}

#[test]
fn watch_prints_what_changed() {
    let env = Env::new();
    let server = Server::start(env.dir.path());
    let mut watcher = Command::new(BIN)
        .args(["--json", "watch", "--type", "Task"])
        .env("STRATA_SOCKET", &server.socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = watcher.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    // the subscription is live once the server has it; add until heard
    let mut heard = None;
    for _ in 0..50 {
        env.ok(
            &["item", "add", "Task"],
            Some(json!({"title": "watched", "status": "todo"})),
        );
        if let Ok(line) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            heard = Some(line);
            break;
        }
    }
    let _ = watcher.kill();
    let _ = watcher.wait();
    let event: Value = serde_json::from_str(&heard.expect("an event")).unwrap();
    assert_eq!(event, json!({"kind": "items", "type": "Task"}));
}

#[test]
fn watch_without_a_server_says_so() {
    let env = Env::new();
    let err = env.fails(1, &["watch"], None);
    assert!(err["error"].as_str().unwrap().contains("strata-server"));
}

/// The M6 gate: export, wipe the data dir, import, and the query results
/// match.
#[test]
fn export_wipe_import_gives_the_same_answers() {
    let env = Env::new();
    let tasks = env.ok(
        &["item", "add", "Task"],
        Some(json!([
            {"title": "export | it", "status": "done", "project": "strata"},
            {"title": "wipe\nit", "status": "todo", "due": "2026-10-01"}
        ])),
    );
    let snapshot = env.ok(
        &["-u", "item", "add", "PortfolioSnapshot"],
        Some(json!({"taken_at": "2026-09-19T10:00:00Z", "currency": "EUR", "total": 42,
                    "positions": [{"symbol": "SECRETSYMBOL", "quantity": 1, "price": 42, "value": 42}]})),
    );
    let (task, snap) = (
        tasks[0]["id"].as_str().unwrap(),
        snapshot["id"].as_str().unwrap(),
    );
    env.ok(&["-u", "item", "relate", snap, task, "funds"], None);
    env.ok(&["item", "relate", task, snap, "about"], None);

    let answers = |env: &Env| {
        json!([
            env.ok(&["-u", "query", "-t", "Task"], None),
            env.ok(&["-u", "query", "-t", "PortfolioSnapshot"], None),
            env.ok(&["-u", "item", "relations", task], None),
            env.ok(&["type", "list"], None),
        ])
    };
    let before = answers(&env);

    let out = env.dir.path().join("export");
    let out_arg = out.to_str().unwrap();
    // locked, the vault's types stay behind
    let locked = env.ok(&["export", "--out", out_arg], None);
    assert_eq!(locked["skipped"], json!(["PortfolioSnapshot"]));
    assert!(!out.join("vault.json.age").exists());
    let done = env.ok(&["-u", "export", "--out", out_arg], None);
    assert_eq!(done["open"], json!(["Task"]));
    assert_eq!(done["vault"], json!(["PortfolioSnapshot"]));

    // nothing of the vault in the clear, and a SuperHub note to read
    for entry in std::fs::read_dir(&out).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("SECRETSYMBOL"));
    }
    let md = std::fs::read_to_string(out.join("Task.md")).unwrap();
    assert!(md.starts_with("---\ntitle: \"strata: Task\"\ntype: reference\n"));
    assert!(md.contains("author: test\n"));
    assert!(md.contains("export \\| it") && md.contains("wipe<br>it"));

    // wipe, start again, bring it back
    std::fs::remove_dir_all(env.dir.path().join("store")).unwrap();
    env.ok(&["init"], None);
    env.fails(2, &["import", out_arg], None);
    let imported = env.ok(&["-u", "import", out_arg], None);
    assert_eq!(imported["items"], 3);
    assert_eq!(imported["relations"], 2);
    assert_eq!(answers(&env), before);

    // a second export keeps the note's created date
    let created = |md: &str| {
        md.lines()
            .find(|l| l.starts_with("created:"))
            .unwrap()
            .to_string()
    };
    env.ok(&["export", "--out", out_arg], None);
    let again = std::fs::read_to_string(out.join("Task.md")).unwrap();
    assert_eq!(created(&again), created(&md));
}

#[test]
fn export_goes_to_the_superhub_vault_by_default() {
    let mut env = Env::new();
    let vault = env.dir.path().join("superhub");
    env.config = Some(format!("superhub_vault = \"{}\"\n", vault.display()));
    let done = env.ok(&["export"], None);
    assert_eq!(done["dir"], json!(vault.join("References/strata")));
    assert!(vault.join("References/strata/Task.md").exists());
    env.config = None;
    let err = env.fails(1, &["export"], None);
    assert!(err["error"].as_str().unwrap().contains("--out"));
}

/// A task that names a note, a note that links it, and one that links
/// nothing that exists: `strata links` reports both directions.
fn links_fixture(env: &Env) -> (String, String) {
    let task = env.ok(
        &["item", "add", "Task"],
        Some(json!({"title": "bridge", "status": "doing", "note": "Projects/strata.md"})),
    );
    let task = task["id"].as_str().unwrap().to_string();
    let gone = "01a0b8c6-0000-7000-8000-000000000000".to_string();
    (task, gone)
}

#[test]
fn links_read_notes_from_a_vault_on_disk() {
    let env = Env::new();
    let (task, gone) = links_fixture(&env);
    let vault = env.dir.path().join("superhub");
    std::fs::create_dir_all(vault.join("Daily")).unwrap();
    std::fs::create_dir_all(vault.join(".git")).unwrap();
    std::fs::write(
        vault.join("Daily/2026-09-19.md"),
        format!("- [ ] push the [bridge](strata://item/{task})\n- old strata://item/{gone}\n"),
    )
    .unwrap();
    std::fs::write(
        vault.join(".git/ignored.md"),
        format!("strata://item/{task}"),
    )
    .unwrap();

    let found = env.ok(&["links", "--vault", vault.to_str().unwrap()], None);
    assert_eq!(
        found["items"],
        json!([{"id": task, "type": "Task", "note": "Projects/strata.md"}])
    );
    assert_eq!(
        found["notes"],
        json!([
            {"note": "Daily/2026-09-19.md", "id": gone, "type": null},
            {"note": "Daily/2026-09-19.md", "id": task, "type": "Task"},
        ])
    );
}

#[test]
fn links_ask_the_hub_when_the_vault_is_elsewhere() {
    use std::io::{BufRead, BufReader, Write};
    let env = Env::new();
    let (task, _) = links_fixture(&env);

    // a stand-in for SuperHub's API: search, then one note, key required
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let note =
        json!({"path": "Daily/2026-09-19.md", "content": format!("see strata://item/{task}")});
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut authorized = false;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                authorized |= line.eq_ignore_ascii_case("authorization: Bearer hub-key\r\n");
                if request.is_empty() {
                    request = line;
                }
            }
            let body = if !authorized {
                "{\"error\": \"who are you\"}".to_string()
            } else if request.starts_with("GET /api/search?") && request.contains("mode=regex") {
                json!([{"path": "Daily/2026-09-19.md"}]).to_string()
            } else if request.starts_with("GET /api/notes/Daily/2026-09-19.md ") {
                note.to_string()
            } else {
                "{}".to_string()
            };
            let status = if authorized {
                "200 OK"
            } else {
                "401 Unauthorized"
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });

    let out = Command::new(BIN)
        .args(["--json", "links"])
        .env("STRATA_DIR", env.dir.path().join("store"))
        .env("STRATA_CONFIG", "/dev/null")
        .env_remove("STRATA_SOCKET")
        .env_remove("SUPERHUB_VAULT_PATH")
        .env("XDG_RUNTIME_DIR", env.dir.path())
        .env("SUPERHUB_URL", &url)
        .env("SUPERHUB_API_KEY", "hub-key")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let found: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(found["notes_from"], url.as_str());
    assert_eq!(
        found["notes"],
        json!([{"note": "Daily/2026-09-19.md", "id": task, "type": "Task"}])
    );
}

/// `strata mcp` over stdio: what an LLM sees of the store.
#[test]
fn mcp_shows_open_items_and_never_a_vault_value() {
    use std::io::{BufRead, BufReader, Write};
    let env = Env::new();
    env.ok(
        &["item", "add", "Task"],
        Some(json!([{"title": "open one", "status": "todo"}, {"title": "done one", "status": "done"}])),
    );
    let snapshot = env.ok(
        &["-u", "item", "add", "PortfolioSnapshot"],
        Some(json!({"taken_at": "2026-09-19", "currency": "EUR", "total": 1234, "positions": []})),
    );
    let snapshot = snapshot["id"].as_str().unwrap();

    // unlocked on purpose: MCP must not show the vault even then
    let mut child = Command::new(BIN)
        .args(["-u", "mcp"])
        .env("STRATA_DIR", env.dir.path().join("store"))
        .env("STRATA_CONFIG", "/dev/null")
        .env("STRATA_VAULT_PASSPHRASE", PASS)
        .env_remove("STRATA_SOCKET")
        .env("XDG_RUNTIME_DIR", env.dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    struct Session {
        stdin: std::process::ChildStdin,
        stdout: BufReader<std::process::ChildStdout>,
    }
    impl Session {
        fn send(&mut self, msg: Value) {
            writeln!(self.stdin, "{msg}").unwrap();
        }
        fn ask(&mut self, msg: Value) -> Value {
            self.send(msg);
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        /// A tool's answer: whether it is an error, and its text.
        fn call(&mut self, id: i64, name: &str, args: Value) -> (bool, String) {
            let reply = self.ask(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                                        "params": {"name": name, "arguments": args}}));
            let result = &reply["result"];
            let text = result["content"][0]["text"].as_str().unwrap().to_string();
            (result["isError"].as_bool().unwrap(), text)
        }
    }
    let mut mcp = Session {
        stdin: child.stdin.take().unwrap(),
        stdout: BufReader::new(child.stdout.take().unwrap()),
    };

    let init = mcp.ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                              "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                         "clientInfo": {"name": "test", "version": "0"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "strata");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    // a notification gets no answer: the next line is the ping's
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    assert_eq!(
        mcp.ask(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))["id"],
        2
    );

    let tools = mcp.ask(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["strata_types", "strata_query", "strata_item"]);

    let (err, text) = mcp.call(
        4,
        "strata_query",
        json!({"type": "Task", "filter": {"status": "todo"}}),
    );
    assert!(!err);
    let todo: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(todo.as_array().unwrap().len(), 1);
    assert_eq!(todo[0]["values"]["title"], "open one");

    let (err, text) = mcp.call(5, "strata_query", json!({"type": "PortfolioSnapshot"}));
    assert!(!err);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!([{"id": snapshot, "type": "PortfolioSnapshot", "locked": true}])
    );
    let (err, text) = mcp.call(
        6,
        "strata_query",
        json!({"type": "PortfolioSnapshot", "filter": {"currency": "EUR"}}),
    );
    assert!(err && text.contains("vault"), "{text}");
    let (err, text) = mcp.call(
        7,
        "strata_item",
        json!({"id": format!("strata://item/{snapshot}")}),
    );
    assert!(!err);
    assert!(!text.contains("1234") && !text.contains("EUR"), "{text}");
    let (err, _) = mcp.call(8, "strata_nothing", json!({}));
    assert!(err);
    let unknown = mcp.ask(json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}));
    assert_eq!(unknown["error"]["code"], -32601);

    drop(mcp);
    assert!(child.wait().unwrap().success());
}
