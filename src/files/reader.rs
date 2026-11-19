use crate::{EffectOutcome, Error, ErrorKind, Result};
use reqwest::{Body, multipart::Part};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf, Take};
use tokio_util::io::ReaderStream;

type BoundedReader = Take<Pin<Box<dyn AsyncRead + Send>>>;

struct State {
    reader: Option<BoundedReader>,
    pending: Option<Waker>,
    complete: bool,
}

// The HTTP body may outlive the request future. This guard keeps the request
// in control of the actual reader even when the transport still owns the body.
pub(super) struct ReaderUpload(Arc<Mutex<State>>);

impl ReaderUpload {
    pub(super) fn new(reader: impl AsyncRead + Send + 'static, length: u64) -> Self {
        let reader: Pin<Box<dyn AsyncRead + Send>> = Box::pin(reader);
        Self(Arc::new(Mutex::new(State {
            reader: Some(reader.take(length)),
            pending: None,
            complete: false,
        })))
    }

    pub(super) fn part(&self) -> Part {
        let stream = ReaderStream::with_capacity(BodyReader(self.0.clone()), 8 * 1024);
        // The declared prefix can be u64::MAX. Avoid reqwest's known-length
        // multipart sum, which adds headers and boundaries to that length.
        Part::stream(Body::wrap_stream(stream))
    }

    fn complete(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.complete)
    }
}

impl Drop for ReaderUpload {
    fn drop(&mut self) {
        let (reader, pending) = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            (state.reader.take(), state.pending.take())
        };
        drop(reader);
        if let Some(waker) = pending {
            waker.wake();
        }
    }
}

struct BodyReader(Arc<Mutex<State>>);

impl AsyncRead for BodyReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("upload_reader"))?;
        if state.complete {
            return Poll::Ready(Ok(()));
        }
        let Some(reader) = &mut state.reader else {
            return Poll::Ready(Err(io::ErrorKind::Interrupted.into()));
        };
        if reader.limit() == 0 {
            // ReaderStream asks for EOF only after giving its caller the final
            // nonempty chunk. Keep EOF valid even after the guard drops the reader.
            state.complete = true;
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        match Pin::new(reader).poll_read(cx, buf) {
            Poll::Pending => {
                if !state
                    .pending
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
                {
                    state.pending = Some(cx.waker().clone());
                }
                Poll::Pending
            }
            Poll::Ready(result) => {
                state.pending = None;
                match result {
                    Ok(()) if buf.filled().len() == before => {
                        Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
                    }
                    Ok(()) => Poll::Ready(Ok(())),
                    Err(_) => Poll::Ready(Err(io::Error::other("upload_reader"))),
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct UploadReaders(Vec<ReaderUpload>);

impl UploadReaders {
    pub(super) fn push(&mut self, guard: ReaderUpload) {
        self.0.push(guard);
    }

    pub(crate) fn finish<T>(self, response: Result<T>) -> Result<T> {
        response.and_then(|value| {
            if self.0.iter().all(ReaderUpload::complete) {
                Ok(value)
            } else {
                Err(Error::new(ErrorKind::Io, EffectOutcome::Unknown))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completion_follows_the_last_read_and_survives_guard_drop() {
        let upload = ReaderUpload::new(&b"data"[..], 4);
        let mut body = BodyReader(upload.0.clone());
        let mut bytes = [0; 8];
        assert_eq!(body.read(&mut bytes).await.unwrap(), 4);
        assert_eq!(&bytes[..4], b"data");
        assert!(!upload.complete(), "last read preceded the body's EOF poll");
        assert_eq!(body.read(&mut bytes).await.unwrap(), 0);
        assert!(upload.complete());
        drop(upload);
        assert_eq!(body.read(&mut bytes).await.unwrap(), 0);
    }
}
