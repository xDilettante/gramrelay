use std::{
    error::Error,
    io::{self, IoSlice, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    task::{Context, Poll},
};

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
    middleware::{Next, from_fn},
    response::Response,
    serve::Listener,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader, ReadBuf},
    net::{TcpListener, TcpStream},
};

use super::{
    DOCUMENT, FILENAME, Scenario, frankenstein_client, mock_router, teloxide_client, tg_client,
};

const WARMUP: usize = 100;
const REQUESTS: usize = 4000;

#[derive(Default)]
struct WireBytes {
    read: AtomicU64,
    written: AtomicU64,
}

struct CountingListener {
    listener: TcpListener,
    mock: Arc<super::Mock>,
    bytes: Arc<WireBytes>,
}

impl Listener for CountingListener {
    type Io = CountingSocket;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, address) = Listener::accept(&mut self.listener).await;
        stream.set_nodelay(true).expect("configure loopback socket");
        self.mock.connections.fetch_add(1, Relaxed);
        let socket = CountingSocket {
            stream,
            bytes: self.bytes.clone(),
        };
        (socket, address)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

struct CountingSocket {
    stream: TcpStream,
    bytes: Arc<WireBytes>,
}

impl AsyncRead for CountingSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let prior = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buffer);
        if let Poll::Ready(Ok(())) = result {
            self.bytes
                .read
                .fetch_add((buffer.filled().len() - prior) as u64, Relaxed);
        }
        result
    }
}

impl AsyncWrite for CountingSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, buffer);
        self.record_write(result)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write_vectored(cx, buffers);
        self.record_write(result)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl CountingSocket {
    fn record_write(&self, result: Poll<io::Result<usize>>) -> Poll<io::Result<usize>> {
        if let Poll::Ready(Ok(written)) = result {
            self.bytes.written.fetch_add(written as u64, Relaxed);
        }
        result
    }
}

fn emit(value: Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}")?;
    stdout.flush()
}

pub(super) async fn server() -> Result<(), Box<dyn Error>> {
    let (mock, router) = mock_router();
    let router = router.layer(from_fn(complete_http_body));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let bytes = Arc::new(WireBytes::default());
    let listener = CountingListener {
        listener,
        mock: mock.clone(),
        bytes: bytes.clone(),
    };
    let serving = tokio::spawn(async move { axum::serve(listener, router).await });
    emit(json!({"address": address.to_string()}))?;
    let mut commands = BufReader::new(tokio::io::stdin()).lines();
    while let Some(command) = commands.next_line().await? {
        match command.as_str() {
            "snapshot" => emit(json!({
                "requests": mock.count.load(Relaxed),
                "connections": mock.connections.load(Relaxed),
                "request_bytes": bytes.read.load(Relaxed),
                "response_bytes": bytes.written.load(Relaxed),
            }))?,
            "stop" => break,
            _ => return Err("unknown fixture command".into()),
        }
    }
    serving.abort();
    let _ = serving.await;
    Ok(())
}

async fn complete_http_body(request: Request, next: Next) -> Result<Response, StatusCode> {
    // Multipart parsing may finish at its own delimiter. Observe HTTP-body EOF
    // before any success so the final chunk framing is included in the counts.
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 128 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await)
}

pub(super) async fn client(
    address: &str,
    client: &str,
    method: &str,
) -> Result<(), Box<dyn Error>> {
    let address: SocketAddrV4 = address.parse()?;
    if *address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
        return Err("the profiling fixture requires a local IPv4 endpoint".into());
    }
    let scenario = Scenario::ALL
        .into_iter()
        .find(|scenario| scenario.method() == method)
        .ok_or("unknown method")?;
    let base = format!("http://{address}");
    let selected = match client {
        "tg" => tg_client(&base)?,
        "teloxide" => teloxide_client(&base)?,
        "frankenstein" => frankenstein_client(&base)?,
        _ => return Err("unknown client".into()),
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(FILENAME);
    if scenario == Scenario::SendDocument {
        std::fs::write(&path, DOCUMENT.as_slice())?;
    }
    for _ in 0..WARMUP {
        selected.request(scenario, &path).await?;
    }
    emit(json!({"phase": "ready", "warmup": WARMUP}))?;
    let mut start = [0];
    tokio::io::stdin().read_exact(&mut start).await?;
    if start[0] != b'g' {
        return Err("unexpected client start marker".into());
    }

    #[cfg(feature = "allocation-counts")]
    let before = super::allocations::Snapshot::now();
    #[cfg(not(feature = "allocation-counts"))]
    let before = process_cpu_ns()?;
    for _ in 0..REQUESTS {
        selected.request(scenario, &path).await?;
    }
    #[cfg(feature = "allocation-counts")]
    let totals = super::allocations::Snapshot::now().since(before);
    #[cfg(not(feature = "allocation-counts"))]
    let totals = json!({
        "cpu_ns": process_cpu_ns()?.checked_sub(before).ok_or("CPU clock moved backwards")?
    });
    emit(json!({"requests": REQUESTS, "totals": totals}))?;
    Ok(())
}

#[cfg(all(unix, not(feature = "allocation-counts")))]
fn process_cpu_ns() -> io::Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: the pointer is writable storage for rusage at this platform ABI.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful getrusage initializes the output.
    let usage = unsafe { usage.assume_init() };
    let nanos = |time: libc::timeval| -> Option<u64> {
        let seconds = u64::try_from(time.tv_sec).ok()?;
        let micros = u64::try_from(time.tv_usec).ok()?;
        seconds
            .checked_mul(1_000_000_000)?
            .checked_add(micros.checked_mul(1000)?)
    };
    nanos(usage.ru_utime)
        .zip(nanos(usage.ru_stime))
        .and_then(|(user, kernel)| user.checked_add(kernel))
        .ok_or_else(|| io::Error::other("invalid CPU time"))
}

#[cfg(all(windows, not(feature = "allocation-counts")))]
fn process_cpu_ns() -> io::Result<u64> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };

    let mut times = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: the current-process pseudo-handle is valid and all four output
    // locations are distinct writable FILETIMEs.
    if unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let ticks =
        |time: &FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    ticks(user)
        .checked_add(ticks(kernel))
        .and_then(|value| value.checked_mul(100))
        .ok_or_else(|| io::Error::other("CPU time overflow"))
}
