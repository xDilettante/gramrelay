use super::{
    JournalError, JournalErrorKind, JournalOptions, Result, deadline, initialize, now, secure_file,
    storage,
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot, watch};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;
type Exit = Option<std::result::Result<(), JournalErrorKind>>;

/// One owning thread from open through close. Admission is bounded independently of cancellation.
pub(super) struct Database {
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    pub(super) options: Arc<JournalOptions>,
    worker_id: Arc<str>,
    pub(super) pending: Arc<Semaphore>,
    stopped: watch::Receiver<Exit>,
}
impl Database {
    pub(super) async fn open(path: PathBuf, options: JournalOptions) -> Result<Self> {
        let options = Arc::new(options);
        let config = options.clone();
        let (sender, receiver) = mpsc::sync_channel::<Job>(options.max_pending);
        let (ready, waiting) = oneshot::channel();
        let (exited, stopped) = watch::channel(None);
        std::thread::Builder::new().name("tg-journal".into()).spawn(move || {
            let opened: Result<(Connection,String)> = (|| {
                deadline(now()?,config.lease)?;
                deadline(now()?,config.retention)?;
                let mut random = [0u8;16];
                getrandom::fill(&mut random).map_err(|_|storage())?;
                let worker: String = random.iter().map(|byte|format!("{byte:02x}")).collect();
                secure_file(&path)?;
                let mut connection = Connection::open_with_flags(&path,OpenFlags::SQLITE_OPEN_READ_WRITE|OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_|storage())?;
                connection.busy_timeout(Duration::from_secs(2)).map_err(|_|storage())?;
                initialize(&mut connection)?;
                Ok((connection,worker))
            })();
            match opened {
                Ok((mut connection,worker)) => {
                    let _=ready.send(Ok(worker.clone()));
                    for job in receiver { job(&mut connection); }
                    // No sender remains: no caller can begin an unstarted reservation now.
                    let released = (|| {
                        let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_|storage())?;
                        let time=now()?;
                        transaction.execute(
                        "UPDATE operations SET worker=NULL,lease_until_ms=NULL,revision=revision+1,updated_at_ms=?1 WHERE state='prepared' AND worker=?2",
                        params![time,worker]).map_err(|_|storage())?;
                        transaction.execute("UPDATE update_feeds SET source_worker=NULL,source_until_ms=NULL,source_revision=source_revision+1 WHERE source_worker=?1",[&worker]).map_err(|_|storage())?;
                        transaction.commit().map_err(|_|storage())
                    })();
                    let closed = connection.close().map_err(|_|storage());
                    let _=exited.send(Some(released.and(closed).map_err(|error|error.kind())));
                },
                Err(error) => {
                    let kind = error.kind();
                    let _=ready.send(Err(error));
                    let _=exited.send(Some(Err(kind)));
                }
            }
        }).map_err(|_|storage())?;
        let worker_id = waiting.await.map_err(|_| storage())??;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            worker_id: worker_id.into(),
            pending: Arc::new(Semaphore::new(options.max_pending)),
            options,
            stopped,
        })
    }

    pub(super) async fn call<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection, &JournalOptions, &str) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, waiting) = oneshot::channel();
        {
            let admission = self.sender.lock().map_err(|_| storage())?;
            let sender = admission
                .as_ref()
                .ok_or_else(|| JournalError::new(JournalErrorKind::Closed))?;
            let permit = self
                .pending
                .clone()
                .try_acquire_owned()
                .map_err(|_| JournalError::new(JournalErrorKind::Capacity))?;
            let options = self.options.clone();
            let worker = self.worker_id.clone();
            sender
                .try_send(Box::new(move |connection| {
                    let result = action(connection, &options, &worker);
                    drop(permit);
                    let _ = reply.send(result);
                }))
                .map_err(|_| storage())?;
        }
        waiting.await.map_err(|_| storage())?
    }

    pub(super) async fn close(&self) -> Result<()> {
        self.sender.lock().map_err(|_| storage())?.take();
        let mut stopped = self.stopped.clone();
        loop {
            let status = *stopped.borrow_and_update();
            if let Some(result) = status {
                return result.map_err(JournalError::new);
            }
            stopped.changed().await.map_err(|_| storage())?;
        }
    }
}
