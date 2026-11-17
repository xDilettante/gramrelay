//! Development-only synthetic SDK. Never use real sessions or production journals.
//! Production tg-agent never selects this factory from config or environment.
#[path = "fixtures/mtproto_wire.rs"]
mod wire;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tg_agent::{
    host::Host,
    tools::{ExecutionMode, OutputLimit},
};
use tg_mtproto::test_kit;
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => {
            eprintln!("agent_fixture_failed");
            std::process::ExitCode::from(2)
        }
    }
}
async fn run() -> Result<(), ()> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let [mode, path] = args.as_slice() else {
        return Err(());
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let counts = (calls.clone(), writes.clone());
    let join_mode = mode == "lifecycle";
    let release = Arc::new(tokio::sync::Notify::new());
    let released = release.clone();
    let pending = std::env::var("TG_MT_FIXTURE_MODE")
        .ok()
        .is_some_and(|v| v == "timeout");
    let entered = std::env::var_os("TG_MT_FIXTURE_ENTERED").map(std::path::PathBuf::from);
    let retained = Arc::new(std::sync::Mutex::new(Vec::new()));
    let keep = retained.clone();
    let loading =
        Host::load_with_mtproto_factory(std::path::Path::new(path), move |session, options| {
            let identity = session.remembered_account().unwrap().unwrap();
            let (calls, writes) = counts.clone();
            let entered = entered.clone();
            let released = released.clone();
            let client = test_kit::client(session, options, move |_, bytes| {
                let calls = calls.clone();
                let writes = writes.clone();
                let entered = entered.clone();
                let released = released.clone();
                async move {
                    assert!(
                        calls.fetch_add(1, Ordering::SeqCst) < 128,
                        "bounded synthetic script"
                    );
                    if wire::is_write(&bytes) {
                        writes.fetch_add(1, Ordering::SeqCst);
                        if let Some(path) = entered {
                            tokio::fs::write(path, b"admitted").await.unwrap();
                        }
                        if pending {
                            std::future::pending::<()>().await;
                        }
                        if join_mode {
                            released.notified().await;
                        }
                    }
                    Ok(wire::response(identity, &bytes))
                }
            })?;
            keep.lock().unwrap().push(client.clone());
            Ok(client)
        })
        .await;
    if mode == "startup-failure" {
        if loading.is_ok() || retained.lock().unwrap().is_empty() {
            return Err(());
        }
        if !reopen_probe().await? {
            return Err(());
        }
        println!(
            "{}",
            serde_json::json!({"accepted_clients":retained.lock().unwrap().len(),"reopened_before_exit":true})
        );
        return Ok(());
    }
    let host = loading.map_err(|_| ())?;
    let result = if join_mode {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        tokio::io::stdin()
            .take(tg_agent::tools::ToolRequest::MAX_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ())?;
        let session = host.session().clone();
        let call =
            tokio::spawn(
                async move { session.dispatch_json(&bytes, ExecutionMode::Execute).await },
            );
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while writes.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ())?;
        call.abort();
        let _ = call.await;
        let waiting = tokio::time::timeout(std::time::Duration::from_millis(10), host.close())
            .await
            .is_err();
        release.notify_one();
        println!("{}", serde_json::json!({"close_was_pending":waiting}));
        Ok(())
    } else if mode == "serve" {
        tg_agent::cli::serve_jsonl(
            host.session(),
            tokio::io::stdin(),
            tokio::io::stdout(),
            ExecutionMode::Execute,
            OutputLimit::default(),
        )
        .await
    } else if mode == "mcp" {
        #[cfg(feature = "mcp")]
        {
            tg_agent::mcp::McpServer::new(
                host.session().clone(),
                ExecutionMode::Execute,
                OutputLimit::default(),
            )
            .serve_parts(tokio::io::stdin(), tokio::io::stdout())
            .await
        }
        #[cfg(not(feature = "mcp"))]
        {
            host.close().await.map_err(|_| ())?;
            return Err(());
        }
    } else {
        host.close().await.map_err(|_| ())?;
        return Err(());
    };
    let closed = host.close().await;
    let reopened = reopen_probe().await?;
    if let Some(path) = std::env::var_os("TG_MT_FIXTURE_COUNTS") {
        let v = serde_json::json!({"calls":calls.load(Ordering::SeqCst),"writes":writes.load(Ordering::SeqCst),"reopened_before_exit":reopened});
        tokio::fs::write(path, serde_json::to_vec(&v).unwrap())
            .await
            .map_err(|_| ())?;
    }
    result.and(closed).map_err(|_| ())
}
async fn reopen_probe() -> Result<bool, ()> {
    use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions};
    if let Some(path) = std::env::var_os("TG_MT_FIXTURE_REOPEN") {
        let session = Session::open_existing(
            std::path::Path::new(&path),
            SessionBinding::new("synthetic", 1).unwrap(),
            SessionKey::from_bytes([7; 32]),
            SessionOptions::default(),
        )
        .await
        .map_err(|_| ())?;
        session.close().await.map_err(|_| ())?;
        return Ok(true);
    }
    Ok(false)
}
