//! Actual encrypted transport against a closing loopback socket, never Telegram.
use grammers_client::{
    Client, SenderPool,
    sender::{ConnectionParams, NoRetries},
    session::{SessionData, storages::MemorySession},
    tl,
};
use std::{
    net::{Ipv4Addr, SocketAddrV4, SocketAddrV6},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncReadExt, net::TcpListener};

#[derive(Clone, Copy)]
enum Mode {
    NoRetries,
    Defaults,
    DroppedCaller,
}
async fn attempt(mode: Mode) -> Result<usize, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let connections = Arc::new(AtomicUsize::new(0));
    let bytes = Arc::new(AtomicUsize::new(0));
    let seen = connections.clone();
    let seen_bytes = bytes.clone();
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = tokio::select! { biased;
                _ = &mut stopped => break,
                accepted = listener.accept() => accepted.unwrap(),
            };
            seen.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0; 8192];
            if let Ok(Ok(length)) =
                tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer)).await
            {
                seen_bytes.fetch_add(length, Ordering::SeqCst);
            }
            // Drop the socket without an MTProto response to simulate I/O loss.
        }
    });
    let mut data = SessionData {
        home_dc: 1,
        ..Default::default()
    };
    data.dc_options.retain(|id, _| *id == 1);
    let dc = data.dc_options.get_mut(&1).unwrap();
    dc.ipv4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    dc.ipv6 = SocketAddrV6::new(Ipv4Addr::LOCALHOST.to_ipv6_mapped(), port, 0, 0);
    dc.auth_key = Some([7; 256]);
    let mut config = ConnectionParams::default();
    if !matches!(mode, Mode::Defaults) {
        config.retry_policy = Box::new(NoRetries);
    }
    let SenderPool { runner, handle, .. } =
        SenderPool::with_configuration(Arc::new(MemorySession::from(data)), 1, config);
    let controller = handle.clone();
    let client = Client::new(handle);
    let ping = tl::functions::Ping { ping_id: 42 };
    let mut invocation = Some(Box::pin(client.invoke(&ping)));
    if matches!(mode, Mode::DroppedCaller) {
        // Poll once with no runner: the backend queues the request and waits.
        tokio::select! { biased;
            _ = invocation.as_mut().unwrap() => panic!("unstarted pool cannot complete a request"),
            _ = tokio::task::yield_now() => {}
        }
        drop(invocation.take());
    }
    let mut running = tokio::spawn(runner.run());
    if let Some(invocation) = invocation {
        let result = tokio::time::timeout(Duration::from_secs(8), invocation).await;
        assert!(
            matches!(result, Ok(Err(_))),
            "closing loopback peer must return an I/O failure"
        );
    } else {
        // A connection AFTER dropping the caller proves that Drop did not
        // remove queued work. It does not prove that the application RPC ran.
        tokio::time::timeout(Duration::from_secs(3), async {
            while bytes.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued backend work survives caller cancellation");
    }
    assert!(controller.quit());
    match tokio::time::timeout(Duration::from_secs(2), &mut running).await {
        Ok(joined) => joined?,
        Err(_) => {
            running.abort();
            let _ = running.await;
            panic!("pool failed joined shutdown");
        }
    }
    let _ = stop.send(());
    server.await?;
    assert!(
        bytes.load(Ordering::SeqCst) > 0,
        "real backend encrypted transport must have written bytes"
    );
    Ok(connections.load(Ordering::SeqCst))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        attempt(Mode::NoRetries).await?,
        1,
        "NoRetries must make one connection"
    );
    assert_eq!(
        attempt(Mode::Defaults).await?,
        2,
        "negative control: default policy retries I/O once"
    );
    assert_eq!(
        attempt(Mode::DroppedCaller).await?,
        1,
        "queued work survives caller Drop"
    );
    println!(
        "grammers loopback: NoRetries=1 connection, default=2 after I/O loss; queued work survives caller Drop; encrypted bytes observed and all pools joined; no public datacenters configured."
    );
    Ok(())
}
