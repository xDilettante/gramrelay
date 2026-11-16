//! Bounded JSON adapters for an explicitly configured trusted-host ToolSession.
//! Standalone execution loads operator configuration through the shared Host.
use crate::{
    executor::Request,
    tools::{ExecutionMode, OutputLimit, ToolError, ToolResponse, ToolSession, encode_response},
};
use std::{io::Write, process::ExitCode};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

pub async fn run_json<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    session: &ToolSession,
    reader: R,
    mut writer: W,
    mode: ExecutionMode,
    limit: OutputLimit,
) -> Result<bool, ToolError> {
    let mut input = Vec::new();
    reader
        .take(Request::MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut input)
        .await
        .map_err(|_| io_error())?;
    if input.len() > Request::MAX_JSON_BYTES {
        let error = oversize();
        emit(
            &mut writer,
            &ToolResponse::Error(error.clone()),
            None,
            limit,
        )
        .await?;
        return Err(error);
    }
    execute_frame(session, &input, &mut writer, mode, limit).await
}
pub async fn serve_jsonl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    session: &ToolSession,
    reader: R,
    mut writer: W,
    mode: ExecutionMode,
    limit: OutputLimit,
) -> Result<(), ToolError> {
    let mut reader = BufReader::with_capacity(8192, reader);
    loop {
        match frame(&mut reader).await {
            Ok(Some(input)) => {
                execute_frame(session, &input, &mut writer, mode, limit).await?;
            }
            Ok(None) => return Ok(()),
            Err(error) => {
                emit(
                    &mut writer,
                    &ToolResponse::Error(error.clone()),
                    None,
                    limit,
                )
                .await?;
                return Err(error);
            }
        }
    }
}
async fn frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, ToolError> {
    let mut input = Vec::new();
    loop {
        let bytes = reader.fill_buf().await.map_err(|_| io_error())?;
        if bytes.is_empty() {
            return Ok((!input.is_empty()).then_some(input));
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(bytes.len());
        if input
            .len()
            .checked_add(count)
            .is_none_or(|length| length > Request::MAX_JSON_BYTES)
        {
            return Err(oversize());
        }
        input.extend_from_slice(&bytes[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(input));
        }
    }
}
async fn execute_frame<W: AsyncWrite + Unpin>(
    session: &ToolSession,
    input: &[u8],
    writer: &mut W,
    mode: ExecutionMode,
    limit: OutputLimit,
) -> Result<bool, ToolError> {
    let request = Request::from_json(input).ok();
    let response = match &request {
        Some(request) => session.execute_for(request.action(), request, mode).await,
        None => ToolResponse::Error(ToolError::invalid_input()),
    };
    emit(writer, &response, request.as_ref(), limit).await
}
async fn emit<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &ToolResponse,
    request: Option<&Request>,
    limit: OutputLimit,
) -> Result<bool, ToolError> {
    let (bytes, is_error) = match encode_response(response, request, limit) {
        Ok(bytes) => (bytes, response.is_error()),
        Err(error) => (
            encode_response(&ToolResponse::Error(error), request, limit)?,
            true,
        ),
    };
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| ToolError::output_io(request))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|_| ToolError::output_io(request))?;
    writer
        .flush()
        .await
        .map_err(|_| ToolError::output_io(request))?;
    Ok(is_error)
}
fn oversize() -> ToolError {
    ToolError::new(
        "agent_input_too_large",
        "Input exceeded 65536 JSON bytes; stream closed.",
    )
}
fn io_error() -> ToolError {
    ToolError::new(
        "agent_io",
        "JSON transport failed. Recover writes using their original operation keys.",
    )
}

/// Thin executable shell; discovery is independent of credentials and storage.
pub fn entrypoint() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "mcp") {
        #[cfg(feature = "mcp")]
        return crate::mcp::entrypoint(&args[1..]);
        #[cfg(not(feature = "mcp"))]
        {
            eprintln!("agent_mcp_feature_required");
            return ExitCode::from(2);
        }
    }
    if let Some((serve, path, mode)) = execution_args(&args) {
        return launch(serve, path, mode);
    }
    let args = args
        .iter()
        .map(|value| value.to_str())
        .collect::<Option<Vec<_>>>();
    let value = match args.as_deref() {
        Some(["--help"] | ["help"]) => {
            println!(
                "tg-agent tools\ntg-agent schema [request|response|catalog]\ntg-agent run --config ABSOLUTE_PATH [--dry-run]\ntg-agent serve --config ABSOLUTE_PATH [--dry-run]\ntg-agent mcp --config ABSOLUTE_PATH [--dry-run] (feature mcp)\n\nDiscovery requires no credentials. Execution uses protected host configuration and environment-only tokens."
            );
            return ExitCode::SUCCESS;
        }
        Some(["tools"] | ["schema", "catalog"]) => serde_json::to_value(crate::tools::catalog()),
        Some(["schema", "request"]) => Ok(crate::tools::request_schema()),
        Some(["schema", "response"]) => Ok(crate::tools::response_schema()),
        Some(["schema"]) => Ok(
            serde_json::json!({"schema_version":1,"request":crate::tools::request_schema(),"response":crate::tools::response_schema(),"catalog":crate::tools::catalog()}),
        ),
        _ => {
            let response = ToolResponse::Error(ToolError::new(
                "agent_usage",
                "Use tools, schema [request|response|catalog], or run/serve --config ABSOLUTE_PATH [--dry-run].",
            ));
            let mut stdout = std::io::stdout().lock();
            let _ = serde_json::to_writer(&mut stdout, &response);
            let _ = stdout.write_all(b"\n");
            return ExitCode::from(2);
        }
    };
    let Ok(value) = value else {
        eprintln!("agent_serialization");
        return ExitCode::from(2);
    };
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer_pretty(&mut stdout, &value).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("agent_io");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
fn execution_args(args: &[std::ffi::OsString]) -> Option<(bool, &std::path::Path, ExecutionMode)> {
    let [command, flag, path, options @ ..] = args else {
        return None;
    };
    if flag != "--config" || (command != "run" && command != "serve") {
        return None;
    }
    let mode = match options {
        [] => ExecutionMode::Execute,
        [flag] if flag == "--dry-run" => ExecutionMode::DryRun,
        _ => return None,
    };
    Some((command == "serve", std::path::Path::new(path), mode))
}
fn launch(serve: bool, path: &std::path::Path, mode: ExecutionMode) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return startup_error(ToolError::new(
                "agent_runtime",
                "Host runtime could not start.",
            ));
        }
    };
    runtime.block_on(async {
        let host = match crate::host::Host::load(path).await {
            Ok(host) => host,
            Err(error) => return startup_error(error),
        };
        let result = if serve {
            serve_jsonl(
                host.session(),
                tokio::io::stdin(),
                tokio::io::stdout(),
                mode,
                OutputLimit::default(),
            )
            .await
            .map(|_| false)
        } else {
            run_json(
                host.session(),
                tokio::io::stdin(),
                tokio::io::stdout(),
                mode,
                OutputLimit::default(),
            )
            .await
        };
        let closed = host.close().await;
        match (result, closed) {
            (Err(error), _) | (_, Err(error)) => {
                eprintln!("{}", error.code());
                ExitCode::from(2)
            }
            (Ok(is_error), Ok(())) => ExitCode::from(u8::from(is_error)),
        }
    })
}
fn startup_error(error: ToolError) -> ExitCode {
    let response = ToolResponse::Error(error);
    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &response).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("agent_io");
    }
    ExitCode::from(2)
}
