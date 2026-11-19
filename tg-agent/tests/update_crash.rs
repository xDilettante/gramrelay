use serde_json::json;
use std::{
    io::{BufRead, Write},
    process::{Command, Stdio},
    time::Duration,
};
use tg_agent::{
    journal::{Journal, JournalErrorKind, JournalOptions},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};

fn event() -> tg::Update {
    serde_json::from_value(json!({"update_id":10,"message":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}})).unwrap()
}
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// Invoked only by the parent test. A kill bypasses every Rust/SQLite destructor.
#[test]
fn update_crash_child_fixture() {
    let Some(path) = std::env::var_os("TG_UPDATE_CRASH_DATABASE") else {
        return;
    };
    let stage = std::env::var("TG_UPDATE_CRASH_STAGE").unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let journal = Journal::open(&path, JournalOptions::default())
                .await
                .unwrap();
            let feed = UpdateFeed::open(journal, "bot", 1, UpdateOptions::default())
                .await
                .unwrap();
            let claim = feed.claim_source(SourceMode::Polling).await;
            if stage == "race" {
                println!(
                    "TG_UPDATE_CLAIM_{}",
                    match &claim {
                        Ok(_) => "READY",
                        Err(error) if error.kind() == JournalErrorKind::Capacity => "BUSY",
                        Err(_) => "ERROR",
                    }
                );
                std::io::stdout().flush().unwrap();
                std::future::pending::<()>().await;
            }
            let source = claim.unwrap();
            let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
            if stage != "received" {
                feed.append_poll(&source, &checkpoint, vec![event()])
                    .await
                    .unwrap();
                if matches!(stage.as_str(), "handled" | "acknowledged") {
                    let consumer = feed.claim_consumer("router").await.unwrap();
                    let (_, ack) = feed
                        .read_consumer(&consumer, 100)
                        .await
                        .unwrap()
                        .into_parts();
                    let mut effect = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(std::path::Path::new(&path).with_extension("effect"))
                        .unwrap();
                    effect.write_all(b"synthetic handler effect").unwrap();
                    effect.sync_all().unwrap();
                    if stage == "acknowledged" {
                        feed.acknowledge_consumer(&consumer, ack).await.unwrap();
                    }
                }
            }
            println!("TG_UPDATE_CRASH_READY");
            std::io::stdout().flush().unwrap();
            std::future::pending::<()>().await;
        });
}

#[test]
fn process_termination_preserves_committed_intake_and_only_committed_consumer_ack() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for stage in ["received", "stored", "handled", "acknowledged"] {
        let path = directory.path().join(format!("{stage}.sqlite"));
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "update_crash_child_fixture", "--nocapture"])
                .env("TG_UPDATE_CRASH_DATABASE", &path)
                .env("TG_UPDATE_CRASH_STAGE", stage)
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
                if line == "TG_UPDATE_CRASH_READY" {
                    let _ = ready.send(());
                    return;
                }
            }
        });
        let observed = waiting.recv_timeout(Duration::from_secs(20));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        reader.join().unwrap();
        observed.unwrap_or_else(|error| panic!("stage {stage} did not reach the barrier: {error}"));
        runtime.block_on(async {
            let journal = Journal::open(&path, JournalOptions::default())
                .await
                .unwrap();
            let feed = UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
                .await
                .unwrap();
            assert_eq!(
                feed.claim_source(SourceMode::Polling)
                    .await
                    .unwrap_err()
                    .kind(),
                JournalErrorKind::Capacity,
                "crashed ownership remains leased"
            );
            // Advance only fixture lease clocks, independently of payload/checkpoint/cursors.
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute("UPDATE update_feeds SET source_until_ms=0", [])
                .unwrap();
            connection
                .execute("UPDATE update_consumers SET lease_until_ms=0", [])
                .unwrap();
            drop(connection);
            let source = feed.claim_source(SourceMode::Polling).await.unwrap();
            let checkpoint = feed.poll_checkpoint(&source).await.unwrap();
            assert_eq!(
                checkpoint.offset(),
                if stage == "received" { None } else { Some(11) }
            );
            if stage == "received" {
                feed.append_poll(&source, &checkpoint, vec![event()])
                    .await
                    .unwrap();
            }
            assert_eq!(feed.read(None, 100).await.unwrap().events().len(), 1);
            let consumer = feed.claim_consumer("router").await.unwrap();
            assert_eq!(
                feed.read_consumer(&consumer, 100)
                    .await
                    .unwrap()
                    .events()
                    .len(),
                usize::from(stage != "acknowledged")
            );
            assert_eq!(
                path.with_extension("effect").exists(),
                matches!(stage, "handled" | "acknowledged")
            );
            journal.close().await.unwrap();
        });
    }
}

#[test]
fn competing_processes_cannot_own_the_same_update_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("race.sqlite");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let journal = Journal::open(&path, JournalOptions::default())
            .await
            .unwrap();
        UpdateFeed::open(journal.clone(), "bot", 1, UpdateOptions::default())
            .await
            .unwrap();
        journal.close().await.unwrap();
    });
    let mut children = Vec::new();
    for _ in 0..2 {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "update_crash_child_fixture", "--nocapture"])
                .env("TG_UPDATE_CRASH_DATABASE", &path)
                .env("TG_UPDATE_CRASH_STAGE", "race")
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
                if line.starts_with("TG_UPDATE_CLAIM_") {
                    let _ = ready.send(line);
                    return;
                }
            }
        });
        children.push((child, waiting, reader));
    }
    let claims: Vec<_> = children
        .iter()
        .map(|(_, waiting, _)| waiting.recv_timeout(Duration::from_secs(20)))
        .collect();
    for (mut child, _, reader) in children {
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
            .filter(|claim| claim.as_str() == "TG_UPDATE_CLAIM_READY")
            .count(),
        1
    );
    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim.as_str() == "TG_UPDATE_CLAIM_BUSY")
            .count(),
        1
    );
}
