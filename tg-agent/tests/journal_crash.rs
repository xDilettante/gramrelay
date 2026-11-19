use rusqlite::Connection;
use serde_json::json;
use std::{
    io::{BufRead, Write},
    process::{Command, Stdio},
    time::Duration,
};
use tg_agent::journal::{
    Completion, Journal, JournalOptions, OperationRequest, OperationState, Preparation,
};

fn request() -> OperationRequest {
    OperationRequest::new(
        "bot",
        "alice",
        "crash-operation",
        "send_message",
        &json!({"chat_id":1,"text":"offline"}),
    )
    .unwrap()
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// The same test executable is the child fixture; normal test runs do no work here.
#[test]
fn crash_child_fixture() {
    let Some(path) = std::env::var_os("TG_AGENT_CRASH_DATABASE") else {
        return;
    };
    let stage = std::env::var("TG_AGENT_CRASH_STAGE").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let journal = Journal::open(&path, JournalOptions::default())
            .await
            .unwrap();
        let reservation = journal.prepare(request()).await.unwrap();
        if stage == "race" {
            println!(
                "TG_AGENT_CLAIM_{}",
                if matches!(reservation, Preparation::Ready(_)) {
                    "READY"
                } else {
                    "EXISTING"
                }
            );
            std::io::stdout().flush().unwrap();
            std::future::pending::<()>().await;
        }
        let Preparation::Ready(lease) = reservation else {
            panic!("expected a new operation");
        };
        if stage != "prepared" {
            let flight = journal.begin(lease).await.unwrap();
            if matches!(stage.as_str(), "sent" | "committed") {
                // A durable synthetic external effect, deliberately separate from SQLite.
                let mut effect = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(std::path::Path::new(&path).with_extension("effect"))
                    .unwrap();
                effect.write_all(b"one simulated external action").unwrap();
                effect.sync_all().unwrap();
            }
            if stage == "committed" {
                journal
                    .complete(flight, Completion::succeeded(json!({"message_id":42})))
                    .await
                    .unwrap();
            }
        }
        println!("TG_AGENT_CRASH_READY");
        std::io::stdout().flush().unwrap();
        std::future::pending::<()>().await;
    });
}

#[test]
fn separate_processes_cannot_both_claim_the_same_operation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("race.sqlite");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    drop(
        runtime
            .block_on(Journal::open(&path, JournalOptions::default()))
            .unwrap(),
    );
    let mut workers = Vec::new();
    for _ in 0..2 {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_child_fixture", "--nocapture"])
                .env("TG_AGENT_CRASH_DATABASE", &path)
                .env("TG_AGENT_CRASH_STAGE", "race")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if line.starts_with("TG_AGENT_CLAIM_") {
                    let _ = ready.send(line);
                    return;
                }
            }
        });
        workers.push((child, waiting, reader));
    }
    let claims: Vec<_> = workers
        .iter()
        .map(|(_, waiting, _)| waiting.recv_timeout(Duration::from_secs(20)))
        .collect();
    for (mut child, _, reader) in workers {
        let _ = child.0.kill();
        child.0.wait().unwrap();
        reader.join().unwrap();
    }
    let claims: Vec<_> = claims
        .into_iter()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim.as_str() == "TG_AGENT_CLAIM_READY")
            .count(),
        1
    );
    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim.as_str() == "TG_AGENT_CLAIM_EXISTING")
            .count(),
        1
    );
}

#[test]
fn terminating_a_real_process_never_replays_an_uncertain_effect() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for stage in ["prepared", "in_flight", "sent", "committed"] {
        let path = directory.path().join(format!("{stage}.sqlite"));
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_child_fixture", "--nocapture"])
                .env("TG_AGENT_CRASH_DATABASE", &path)
                .env("TG_AGENT_CRASH_STAGE", stage)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if line == "TG_AGENT_CRASH_READY" {
                    let _ = ready.send(());
                    return;
                }
            }
        });
        let observed = waiting.recv_timeout(Duration::from_secs(20));
        // Kill bypasses Rust drops and SQLite close/checkpoint cleanup.
        let _ = child.0.kill();
        child.0.wait().unwrap();
        reader.join().unwrap();
        observed
            .unwrap_or_else(|error| panic!("child stage {stage} failed to reach barrier: {error}"));
        runtime.block_on(async {
            let journal = Journal::open(&path,JournalOptions::default()).await.unwrap();
            let before = journal.get("bot","alice","crash-operation").await.unwrap().unwrap();
            if stage == "committed" {
                assert_eq!(before.state(),OperationState::Succeeded);
                assert_eq!(before.result().unwrap()["message_id"],42);
            } else {
                assert_eq!(before.state(),if stage == "prepared" { OperationState::Prepared } else { OperationState::InFlight });
                Connection::open(&path).unwrap().execute("UPDATE operations SET lease_until_ms=0",[]).unwrap();
                journal.recover_expired(10).await.unwrap();
            }
            let replay = journal.prepare(request()).await.unwrap();
            match stage {
                "prepared" => assert!(matches!(replay,Preparation::Ready(_))),
                "in_flight"|"sent" => assert!(matches!(replay,Preparation::Existing(operation) if operation.state() == OperationState::Unknown)),
                _ => assert!(matches!(replay,Preparation::Existing(operation) if operation.state() == OperationState::Succeeded)),
            }
            let effect = path.with_extension("effect");
            if matches!(stage,"sent"|"committed") { assert_eq!(std::fs::read(effect).unwrap(),b"one simulated external action"); }
            else { assert!(!effect.exists()); }
        });
    }
}
