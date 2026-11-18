use std::{
    error::Error,
    process::{self, Command},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    time::Duration,
};

use axum::{Router, extract::State, http::header::CONTENT_TYPE, routing::post};
use serde_json::json;
use tg::{Bot, EffectOutcome, ErrorKind};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinSet,
    time::timeout,
};

const CAP: usize = 8;
const BURST: usize = 128;
const ROUNDS: usize = 5;
const TOKEN: &str = "0:PRESSURE";
const REPLY: &str = r#"{"ok":true,"result":{"id":7,"is_bot":true,"first_name":"Pressure","username":"pressure_bot"}}"#;

struct Mock {
    permits: Semaphore,
    arrival: Notify,
    received: AtomicUsize,
    active: AtomicUsize,
    peak_active: AtomicUsize,
}

async fn hold(
    State(mock): State<Arc<Mock>>,
) -> ([(axum::http::HeaderName, &'static str); 1], &'static str) {
    let active = mock.active.fetch_add(1, Relaxed) + 1;
    mock.peak_active.fetch_max(active, Relaxed);
    mock.received.fetch_add(1, Relaxed);
    mock.arrival.notify_one();
    let _permit = mock
        .permits
        .acquire()
        .await
        .expect("fixture semaphore is never closed");
    mock.active.fetch_sub(1, Relaxed);
    ([(CONTENT_TYPE, "application/json")], REPLY)
}

fn resident_bytes() -> Result<u64, Box<dyn Error>> {
    let pid = process::id();
    #[cfg(windows)]
    let (value, multiplier) = (
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-Process -Id {pid} -ErrorAction Stop).WorkingSet64"),
            ])
            .output()?,
        1,
    );
    #[cfg(not(windows))]
    let (value, multiplier) = (
        Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()?,
        1024,
    );
    if !value.status.success() {
        return Err("unable to sample process resident memory".into());
    }
    Ok(std::str::from_utf8(&value.stdout)?.trim().parse::<u64>()? * multiplier)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mock = Arc::new(Mock {
        permits: Semaphore::new(CAP),
        arrival: Notify::new(),
        received: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        peak_active: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .route(&format!("/bot{TOKEN}/getMe"), post(hold))
        .with_state(Arc::clone(&mock));
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let bot = Bot::builder(TOKEN)
        .api_url(base)
        .allow_http_loopback(true)
        .max_in_flight(CAP)
        .timeout(Duration::from_secs(30))
        .build()?;
    bot.api().get_me().await?;
    let baseline_bytes = resident_bytes()?;
    let mut trials = Vec::new();
    for round in 1..=ROUNDS {
        mock.permits.acquire_many(CAP as u32).await?.forget();
        let prior = mock.received.load(Relaxed);
        let mut tasks = JoinSet::new();
        for _ in 0..BURST {
            let bot = bot.clone();
            tasks.spawn(async move { bot.api().get_me().await });
        }
        let deadline = Duration::from_secs(10);
        for _ in 0..BURST - CAP {
            let result = timeout(deadline, tasks.join_next())
                .await?
                .ok_or("missing result")??;
            match result {
                Err(error)
                    if error.kind() == ErrorKind::Capacity
                        && error.outcome() == EffectOutcome::NotSent => {}
                _ => return Err("expected local capacity rejection before release".into()),
            }
        }
        timeout(deadline, async {
            while mock.received.load(Relaxed) < prior + CAP {
                mock.arrival.notified().await;
            }
        })
        .await?;
        if mock.received.load(Relaxed) != prior + CAP || mock.active.load(Relaxed) != CAP {
            return Err("HTTP calls exceeded the configured capacity".into());
        }
        let saturated_bytes = resident_bytes()?;
        mock.permits.add_permits(CAP);
        for _ in 0..CAP {
            timeout(deadline, tasks.join_next())
                .await?
                .ok_or("missing admitted result")???;
        }
        if !tasks.is_empty() || mock.active.load(Relaxed) != 0 {
            return Err("HTTP work did not settle".into());
        }
        bot.api().get_me().await?;
        if mock.received.load(Relaxed) != prior + CAP + 1 {
            return Err("unexpected additional dispatch during recovery".into());
        }
        trials.push(json!({
            "round": round, "admitted": CAP, "capacity_not_sent": BURST - CAP,
            "recovered": true, "saturated_resident_bytes": saturated_bytes,
            "settled_resident_bytes": resident_bytes()?
        }));
    }
    server.abort();
    let _ = server.await;
    println!(
        "{}",
        json!({
            "http_limit": CAP, "caller_burst": BURST, "peak_server_requests": mock.peak_active.load(Relaxed),
            "baseline_resident_bytes": baseline_bytes, "trials": trials
        })
    );
    Ok(())
}
