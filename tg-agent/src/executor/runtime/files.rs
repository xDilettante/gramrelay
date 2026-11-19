use super::*;
pub use crate::executor::file_store::{FileLimits, RegisteredFile};
pub(super) use crate::executor::file_store::{FileRegistry, ensure_live, remaining};
impl Executor {
    fn file_profile(&self, profile: &str) -> Result<Arc<Profile>> {
        valid_id(profile)?;
        self.profiles
            .read()
            .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
            .get(profile)
            .cloned()
            .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Denied))
    }
    /// Trusted-host registration. This creates no agent grant or approval.
    pub async fn register_file(
        &self,
        principal: &Principal,
        profile: &str,
        filename: impl Into<String>,
        bytes: Vec<u8>,
        expires_at_ms: i64,
    ) -> Result<RegisteredFile> {
        let source = self.file_profile(profile)?;
        self.files
            .register_file(
                &source.journal,
                principal,
                profile,
                filename,
                bytes,
                expires_at_ms,
            )
            .await
    }
    /// Reattaches only identical owned bytes; immutable metadata is never replaced.
    pub async fn restore_file(
        &self,
        principal: &Principal,
        profile: &str,
        handle: &str,
        bytes: Vec<u8>,
    ) -> Result<RegisteredFile> {
        let source = self.file_profile(profile)?;
        self.files
            .restore_file(&source.journal, principal, profile, handle, bytes)
            .await
    }
    /// Discards resident bytes. Outstanding owners retain their memory permits.
    pub fn discard_file(&self, principal: &Principal, profile: &str, handle: &str) -> Result<bool> {
        self.files.discard_file(principal, profile, handle)
    }
    pub fn prune_files(&self) -> Result<usize> {
        self.files.prune_files()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{journal::JournalOptions, updates::UpdateOptions};
    use std::{future::Future, task::Poll};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pending_restore_reserves_snapshot_memory_before_waiting_for_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("files.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let limits = FileLimits {
            max_file_bytes: 1,
            max_snapshot_bytes: 1,
            max_snapshots: 1,
            max_upload_bytes: 1,
        };
        let executor = Executor::new(
            PolicyStore::default(),
            ExecutorOptions::default().file_limits(limits),
        )
        .unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                tg::test_kit::MockTransport::new([]).unwrap().bot().unwrap(),
            )
            .unwrap();
        let principal = Principal::new("alice").unwrap();
        let file = executor
            .register_file(
                &principal,
                "primary",
                "one.bin",
                vec![1],
                now().unwrap() + 60000,
            )
            .await
            .unwrap();
        executor
            .discard_file(&principal, "primary", file.handle())
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked = journal.clone();
        let gate = tokio::spawn(async move {
            blocked
                .run(move |_, _, _| {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        let first = executor.restore_file(&principal, "primary", file.handle(), vec![1]);
        tokio::pin!(first);
        std::future::poll_fn(|cx| match first.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("metadata must wait for database gate"),
        })
        .await;
        let second = timeout(
            Duration::from_millis(40),
            executor.restore_file(&principal, "primary", file.handle(), vec![1]),
        )
        .await;
        release_tx.send(()).unwrap();
        gate.await.unwrap().unwrap();
        first.await.unwrap();
        assert_eq!(
            second
                .expect("pending restore must reserve capacity before metadata wait")
                .unwrap_err()
                .kind(),
            ExecutorErrorKind::Capacity
        );
        journal.close().await.unwrap();
    }

    #[tokio::test]
    async fn expired_document_cannot_start_identity_after_waiting_for_another_initializer() {
        use tg::test_kit::{MockResponse, MockTransport, Step};
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            directory.path().join("identity.sqlite"),
            JournalOptions::default(),
        )
        .await
        .unwrap();
        let policy = PolicyStore::default();
        let principal = Principal::new("alice").unwrap();
        let mock=MockTransport::new([Step::reply("getMe",MockResponse::json(serde_json::json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}})))]).unwrap();
        let executor = Executor::new(policy.clone(), ExecutorOptions::default()).unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal.clone(), "primary", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                mock.bot().unwrap(),
            )
            .unwrap();
        let file = executor
            .register_file(
                &principal,
                "primary",
                "one.bin",
                vec![1],
                now().unwrap() + 1000,
            )
            .await
            .unwrap();
        policy
            .replace(
                &principal,
                "primary",
                Grant::new(0, Budget::new("quota").unwrap())
                    .allow(Action::SendDocument)
                    .chats(ChatScope::only([ChatId::new(1).unwrap()]))
                    .files([file.handle()]),
            )
            .unwrap();
        let request=Request::from_json(&serde_json::to_vec(&serde_json::json!({"schema_version":1,"profile":"primary","operation_id":"one","call":{"tool":"send_document","arguments":{"chat_id":"1","file_handle":file.handle()}}})).unwrap()).unwrap();
        let profile = executor.file_profile("primary").unwrap();
        let auth = executor
            .read_authorization(&profile, &principal, &request, None)
            .await
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let held = profile.clone();
        let gate = tokio::spawn(async move {
            let _ = held
                .identity
                .get_or_try_init(|| async {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    Err::<BotInfo, _>(ExecutorError::new(ExecutorErrorKind::Identity))
                })
                .await;
        });
        started_rx.await.unwrap();
        let verified = executor.verified(&profile, &auth);
        tokio::pin!(verified);
        std::future::poll_fn(|cx| match verified.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("identity must wait"),
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        release_tx.send(()).unwrap();
        gate.await.unwrap();
        let result = verified.await;
        assert!(
            mock.requests().unwrap().is_empty(),
            "expired document must not start getMe"
        );
        let Err(error) = result else {
            panic!("expired file must fail")
        };
        assert_eq!(error.kind(), ExecutorErrorKind::FileExpired);
        journal.close().await.unwrap();
    }
}
