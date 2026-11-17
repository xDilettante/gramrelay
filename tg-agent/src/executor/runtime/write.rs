use super::*;

impl Executor {
    pub(super) async fn execute_write(
        &self,
        profile: &Profile,
        principal: &Principal,
        request: &Request,
        approval: Option<&Approval>,
    ) -> Result<ExecutionResult> {
        let invocation = self
            .read_authorization(profile, principal, request, approval)
            .await?;
        invocation.checked(now()?)?;
        let admission = invocation.clone();
        profile
            .journal
            .charge_authorized_invocation(move |time| Ok((invocation.budget_at(time)?, 0)))
            .await
            .map_err(storage_error)?;
        admission.checked(now()?)?;
        let scope = match request.call() {
            ActionCall::SendMessage(args) => OperationScope {
                account: profile.expected,
                action: request.action(),
                chat: Some(args.chat_id),
                file: None,
                reference_hash: None,
            },
            ActionCall::EditMessageText(args) => OperationScope {
                account: profile.expected,
                action: request.action(),
                chat: Some(args.chat_id),
                file: None,
                reference_hash: None,
            },
            ActionCall::AnswerCallbackQuery(_) => {
                admission.scope.clone().expect("registered callback scope")
            }
            ActionCall::SendDocument(args) => {
                let file = admission.file.as_ref().expect("resolved file metadata");
                OperationScope {
                    account: profile.expected,
                    action: Action::SendDocument,
                    chat: Some(args.chat_id),
                    file: Some(journal::authorization::FileScope {
                        handle: file.handle.clone(),
                        sha256: file.sha256,
                        bytes: file.byte_size,
                    }),
                    reference_hash: None,
                }
            }
            _ => return Err(ExecutorError::new(ExecutorErrorKind::UnsupportedAction)),
        };
        let auth = admission.clone();
        let payload = match &auth.reference {
            Some(reference) => {
                serde_json::json!({"request":request,"reference":{"received_at_ms":reference.received_at_ms,"expires_at_ms":reference.expires_at_ms}})
            }
            None => match &auth.file {
                Some(file) => {
                    serde_json::json!({"request":request,"file":{"handle":file.handle,"sha256":file.sha256,"byte_size":file.byte_size,"filename":file.filename,"expires_at_ms":file.expires_at_ms}})
                }
                None => serde_json::to_value(request)
                    .map_err(|_| ExecutorError::new(ExecutorErrorKind::InvalidInput))?,
            },
        };
        let intent = OperationRequest::new(
            request.profile(),
            principal.as_str(),
            request.operation_id().expect("validated write ID"),
            request.action().name(),
            &payload,
        )
        .map_err(storage_error)?;
        let prepared = match profile
            .journal
            .prepare_scoped(intent, scope.clone())
            .await
            .map_err(storage_error)?
        {
            Preparation::Ready(value) => value,
            Preparation::Existing(_) => {
                return self
                    .cached(profile, &auth, &scope)
                    .await
                    .map_err(|error| error.with_outcome(EffectOutcome::Unknown));
            }
        };
        let upload = match &auth.file {
            Some(file) => match self.files.upload(file) {
                Ok(upload) => Some(upload),
                Err(error) => {
                    let _ = profile.journal.release_prepared(prepared).await;
                    return Err(error);
                }
            },
            None => None,
        };
        let identity = async {
            let mut identity_budget = auth
                .reference
                .as_ref()
                .map(|reference| reference.remaining(now()?))
                .transpose()?
                .map_or(self.options.request_budget, |value| {
                    value.min(self.options.request_budget)
                });
            if let Some(file) = &auth.file {
                identity_budget =
                    identity_budget.min(files::remaining(file.expires_at_ms, now()?)?);
            }
            timeout(identity_budget, self.verified(profile, &auth))
                .await
                .map_err(|_| {
                    ExecutorError::new(if auth.reference.is_some() {
                        ExecutorErrorKind::ReferenceExpired
                    } else if auth.file.as_ref().is_some_and(|file| {
                        super::now().is_ok_and(|time| time >= file.expires_at_ms)
                    }) {
                        ExecutorErrorKind::FileExpired
                    } else {
                        ExecutorErrorKind::Identity
                    })
                })?
        }
        .await;
        if let Err(error) = identity {
            let _ = profile.journal.release_prepared(prepared).await;
            return Err(error);
        }
        let proof = prepared.clone();
        let flight = match profile
            .journal
            .begin_authorized(prepared, move |time| admission.write_budget_at(time))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = profile.journal.release_prepared(proof).await;
                return Err(storage_error(error));
            }
        };
        // Anchor before reading UTC so later policy waits cannot add time back.
        let sampled = Instant::now();
        let remaining = flight
            .lease_until_ms()
            .checked_sub(now()?)
            .and_then(|v| u64::try_from(v).ok())
            .map(Duration::from_millis)
            .unwrap_or_default();
        let lease_deadline = sampled.checked_add(remaining);
        let rpc = (|| -> Result<Instant> {
            auth.write_checked(now()?)?;
            let now = Instant::now();
            let lease_limit = lease_deadline
                .and_then(|deadline| deadline.checked_sub(self.options.receipt_margin))
                .filter(|deadline| *deadline > now)
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::Capacity))?;
            let deadline = now
                .checked_add(self.options.request_budget)
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?
                .min(lease_limit);
            let deadline = auth
                .reference
                .as_ref()
                .map_or(deadline, |reference| reference.cap_rpc(deadline));
            let deadline = if let Some(file) = &auth.file {
                let sampled = Instant::now();
                let remaining = files::remaining(file.expires_at_ms, super::now()?)?;
                deadline.min(
                    sampled
                        .checked_add(remaining)
                        .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::InvalidPolicy))?,
                )
            } else {
                deadline
            };
            Ok(deadline)
        })();
        let rpc_deadline = match rpc {
            Ok(value) => value,
            Err(error) => {
                let _ = profile
                    .journal
                    .complete_fenced(flight, Completion::executor_not_started())
                    .await;
                return Err(error.with_outcome(EffectOutcome::NotSent));
            }
        };
        let Some(end) = rpc_deadline
            .checked_add(self.options.receipt_margin)
            .zip(lease_deadline)
            .map(|(deadline, lease)| deadline.min(lease))
        else {
            let _ = profile
                .journal
                .complete_fenced(flight, Completion::executor_not_started())
                .await;
            return Err(ExecutorError::new(ExecutorErrorKind::InvalidPolicy)
                .with_outcome(EffectOutcome::NotSent));
        };
        let completion = match request.call() {
            ActionCall::SendMessage(args) => {
                let chat = args.chat_id;
                let mut params = tg::api::methods::SendMessage::new(args.text.clone())
                    .chat_id(tg::ChatId::from(chat.get()).into());
                if let Some(id) = args.message_thread_id {
                    params = params.message_thread_id(id.get());
                }
                if let Some(value) = args.disable_notification {
                    params = params.disable_notification(value);
                }
                attempt(&profile.bot, params, rpc_deadline, &auth, None, |message| {
                    message_value(&message, chat, None)
                })
                .await
            }
            ActionCall::EditMessageText(args) => {
                let chat = args.chat_id;
                let params = tg::api::methods::EditMessageText::new()
                    .chat_id(tg::ChatId::from(chat.get()).into())
                    .message_id(tg::MessageId::from(args.message_id.get()))
                    .text(args.text.clone());
                attempt(
                    &profile.bot,
                    params,
                    rpc_deadline,
                    &auth,
                    None,
                    |response| match response {
                        tg::api::types::EditMessageTextResult::Message(message) => {
                            message_value(&message, chat, Some(args.message_id))
                        }
                        _ => None,
                    },
                )
                .await
            }
            ActionCall::AnswerCallbackQuery(args) => {
                let mut params = tg::api::methods::AnswerCallbackQuery::new(
                    args.callback_query_id.clone().into(),
                );
                if let Some(text) = &args.text {
                    params = params.text(text.clone());
                }
                if let Some(alert) = args.show_alert {
                    params = params.show_alert(alert);
                }
                attempt(&profile.bot, params, rpc_deadline, &auth, None, |_| {
                    Some(serde_json::json!({"acknowledged":true}))
                })
                .await
            }
            ActionCall::SendDocument(args) => {
                let chat = args.chat_id;
                let mut params = tg::api::methods::SendDocument::new("attach://file".into())
                    .chat_id(tg::ChatId::from(chat.get()).into());
                if let Some(caption) = &args.caption {
                    params = params.caption(caption.clone());
                }
                attempt(
                    &profile.bot,
                    params,
                    rpc_deadline,
                    &auth,
                    upload,
                    |message| message_value(&message, chat, None),
                )
                .await
            }
            _ => Completion::executor_not_started(),
        };
        timeout_at(end, profile.journal.complete_fenced(flight, completion))
            .await
            .map_err(|_| uncertain())?
            .map_err(|_| uncertain())?;
        self.cached(profile, &auth, &scope)
            .await
            .map_err(|error| error.with_outcome(EffectOutcome::Unknown))
    }
}
async fn attempt<M: tg::api::Method>(
    bot: &Bot,
    params: M,
    deadline: Instant,
    auth: &Authorization,
    attachment: Option<tg::InputFile>,
    project: impl FnOnce(M::Response) -> Option<serde_json::Value> + Send,
) -> Completion {
    let Ok(time) = now() else {
        return Completion::executor_not_started();
    };
    if auth.write_checked(time).is_err() {
        return Completion::executor_not_started();
    }
    let Some(budget) = deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
    else {
        return Completion::executor_not_started();
    };
    let mut call = bot.call(params).retry_policy(
        tg::RetryPolicy::default()
            .max_attempts(1)
            .max_elapsed(budget),
    );
    if let Some(file) = attachment {
        call = call.attach("file", file);
    }
    match timeout_at(deadline, call).await {
        Ok(Ok(response)) => {
            project(response).map_or_else(Completion::executor_uncertain, Completion::succeeded)
        }
        Ok(Err(error)) => Completion::from_error(&error),
        Err(_) => Completion::executor_uncertain(),
    }
}
fn message_value(
    message: &tg::Message,
    chat: ChatId,
    expected: Option<MessageId>,
) -> Option<serde_json::Value> {
    let id = i64::from(message.message_id);
    if i64::from(message.chat.id) != chat.get()
        || id <= 0
        || expected.is_some_and(|v| v.get() != id)
    {
        return None;
    }
    Some(serde_json::json!({"chat_id":chat,"message_id":id.to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        future::Future,
        sync::mpsc,
        task::{Context, Poll, Wake, Waker},
    };
    use tg::test_kit::{MockResponse, MockTransport, Step};
    struct Wakeup(tokio::sync::Notify);
    impl Wake for Wakeup {
        fn wake(self: Arc<Self>) {
            self.0.notify_one();
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn policy_wait_after_begin_does_not_extend_the_operation_lease() {
        use crate::{journal::JournalOptions, updates::UpdateOptions};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.sqlite");
        let journal = Journal::open(
            &path,
            JournalOptions::default().lease_duration(Duration::from_millis(300)),
        )
        .await
        .unwrap();
        let policy = PolicyStore::default();
        let principal = Principal::new("alice").unwrap();
        policy
            .replace(
                &principal,
                "primary",
                Grant::new(0, Budget::new("quota").unwrap())
                    .allow(Action::GetMe)
                    .allow(Action::SendMessage)
                    .chats(ChatScope::All),
            )
            .unwrap();
        let mock=MockTransport::new([Step::reply("getMe",MockResponse::json(json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Offline"}}))),Step::reply("sendMessage",MockResponse::json(json!({"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}})))]).unwrap();
        let executor = Executor::new(
            policy.clone(),
            ExecutorOptions::default()
                .request_budget(Duration::from_millis(200))
                .receipt_margin(Duration::from_millis(100)),
        )
        .unwrap();
        executor
            .register_profile(
                UpdateFeed::open(journal, "primary", 0, UpdateOptions::default())
                    .await
                    .unwrap(),
                mock.bot().unwrap(),
            )
            .unwrap();
        let me = Request::from_json(
            br#"{"schema_version":1,"profile":"primary","call":{"tool":"get_me","arguments":{}}}"#,
        )
        .unwrap();
        executor.execute(&principal, &me, None).await.unwrap();
        let request=Request::from_json(br#"{"schema_version":1,"profile":"primary","operation_id":"lease-bound","call":{"tool":"send_message","arguments":{"chat_id":"1","text":"hello"}}}"#).unwrap();
        let mut future =
            Box::pin(async move { executor.execute(&principal, &request, None).await });
        let wake = Arc::new(Wakeup(tokio::sync::Notify::new()));
        let waker = Waker::from(wake.clone());
        let mut cx = Context::from_waker(&waker);
        let blocker = rusqlite::Connection::open(&path).unwrap();
        // Warm identity makes the next three awaits invocation, prepare and begin.
        for _ in 0..3 {
            blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
            blocker.execute_batch("ROLLBACK").unwrap();
            tokio::time::timeout(Duration::from_secs(2), wake.0.notified())
                .await
                .unwrap();
        }
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = std::thread::spawn(move || {
            policy.with_exclusive_test_lock(|| {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        held_rx.recv().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut started = Some(started_tx);
        let task = tokio::spawn(std::future::poll_fn(move |cx| {
            if let Some(signal) = started.take() {
                let _ = signal.send(());
            }
            future.as_mut().poll(cx)
        }));
        started_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        release_tx.send(()).unwrap();
        gate.join().unwrap();
        let _ = task.await.unwrap();
        assert_eq!(
            mock.requests().unwrap().len(),
            1,
            "an expired lease must not dispatch sendMessage"
        );
    }
}
