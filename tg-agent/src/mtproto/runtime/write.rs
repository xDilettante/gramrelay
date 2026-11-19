use super::*;
impl Worker {
    pub(super) async fn write(&self, auth: &Authorization) -> Result<ExecutionResult> {
        let request = &auth.request;
        let scope = if request.action() == Action::AnswerCallbackQuery {
            auth.scope
                .clone()
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::ContextRequired))?
        } else {
            Scope {
                account: self.profile.expected,
                action: request.action(),
                peer: request.call().peer(),
                file: auth
                    .file
                    .as_ref()
                    .map(|f| journal::authorization::FileScope {
                        handle: f.handle.clone(),
                        sha256: f.sha256,
                        bytes: f.byte_size,
                    }),
                reference_hash: None,
            }
        };
        let payload = if let Some(reference) = &auth.reference {
            callbacks::payload(request, reference)?
        } else if let Some(file) = &auth.file {
            serde_json::json!({"request":request,"file":{"handle":file.handle,"sha256":file.sha256,"byte_size":file.byte_size,"filename":file.filename,"expires_at_ms":file.expires_at_ms}})
        } else {
            serde_json::to_value(request).map_err(|_| invalid_policy())?
        };
        let operation = OperationRequest::new(
            &self.profile.name,
            auth.principal.as_str(),
            request.operation_id().expect("write key"),
            request.action().name(),
            &payload,
        )
        .map_err(storage_error)?;
        let prepared = match self
            .profile
            .journal
            .prepare_mtproto(operation, scope.clone())
            .await
            .map_err(storage_error)?
        {
            Preparation::Ready(p) => p,
            Preparation::Existing(_) => {
                return self
                    .cached(auth, &scope)
                    .await
                    .map_err(|e| e.with_outcome(EffectOutcome::Unknown));
            }
        };
        let preparation = async {
            auth.write_checked(now()?)?;
            if let Call::AnswerCallbackQuery(args) = request.call() {
                let reference = auth
                    .reference
                    .as_ref()
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::ContextRequired))?;
                let mut intent =
                    tg_mtproto::CallbackAnswer::new(reference.query).show_alert(args.show_alert);
                if let Some(text) = &args.text {
                    intent = intent.text(text.clone()).map_err(remote)?;
                }
                return Ok(Input::Callback(intent));
            }
            if let Call::SendDocument(args) = request.call() {
                let file = auth.file.as_ref().expect("resolved file metadata");
                let owner = self.files.upload_owner(file)?;
                let mut intent = tg_mtproto::SendDocument::owned_bytes(
                    file.filename.clone(),
                    "application/octet-stream",
                    owner,
                )
                .map_err(remote)?
                .expected_sha256(file.sha256)
                .max_bytes(file.byte_size);
                if let Some(caption) = &args.caption {
                    intent = intent.caption(caption.clone()).map_err(remote)?;
                }
                let deadline = files::cap(
                    file,
                    Instant::now(),
                    now()?,
                    self.profile.client.request_budget(),
                )?;
                let peer = self
                    .profile
                    .account
                    .cached_peer_until(args.peer.as_client_id(), deadline)
                    .await
                    .map_err(remote)?
                    .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
                return Ok(Input::Document(peer, intent));
            }
            let intent = if let Call::SendMessage(args) = request.call() {
                Some(
                    tg_mtproto::SendText::new(args.text.clone())
                        .map_err(remote)?
                        .silent(args.disable_notification),
                )
            } else {
                None
            };
            let resolved = self
                .profile
                .account
                .cached_peer(scope.peer.expect("text peer").as_client_id())
                .await
                .map_err(remote)?
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
            Ok(Input::Text(resolved, intent))
        }
        .await;
        let input = match preparation {
            Ok(peer) => peer,
            Err(error) => {
                let _ = self.profile.journal.release_prepared(prepared).await;
                return Err(error);
            }
        };
        let proof = prepared.clone();
        let admission = auth.clone();
        let flight = match self
            .profile
            .journal
            .begin_mtproto(prepared, scope.clone(), move |time| {
                admission.write_budget(time)
            })
            .await
        {
            Ok(flight) => flight,
            Err(error) => {
                let _ = self.profile.journal.release_prepared(proof).await;
                return Err(storage_error(error));
            }
        };
        let dispatch = rpc_deadline(&flight, self.options.receipt_margin, auth);
        let deadline = match dispatch {
            Ok(d) => d,
            Err(error) => {
                self.profile
                    .journal
                    .complete_fenced(flight, Completion::executor_not_started())
                    .await
                    .map_err(|_| uncertain())?;
                return Err(error.with_outcome(EffectOutcome::NotSent));
            }
        };
        let response = match (input, request.call()) {
            (Input::Document(peer, intent), Call::SendDocument(_)) => self
                .profile
                .account
                .send_document_until(&peer, intent, deadline)
                .await
                .map(|receipt| {
                    let file = auth.file.as_ref().expect("resolved file metadata");
                    if receipt.size() != file.byte_size || receipt.sha256() != file.sha256 {
                        None
                    } else {
                        message_value(receipt.message(), scope.peer, None)
                    }
                }),
            (Input::Callback(intent), Call::AnswerCallbackQuery(_)) => self
                .profile
                .account
                .answer_callback_until(intent, deadline)
                .await
                .map(|_| Some(serde_json::json!({"acknowledged":true}))),
            (Input::Text(resolved, intent), Call::SendMessage(_)) => self
                .profile
                .account
                .send_text_until(&resolved, intent.expect("send intent"), deadline)
                .await
                .map(|r| message_value(r, scope.peer, None)),
            (Input::Text(resolved, _), Call::EditMessageText(args)) => self
                .profile
                .account
                .edit_text_until(
                    &resolved,
                    args.message_id.get() as i32,
                    args.text.clone(),
                    deadline,
                )
                .await
                .map(|r| message_value(r, scope.peer, Some(args.message_id.get()))),
            _ => unreachable!("prepared write action"),
        };
        let completion = match response {
            Ok(Some(value)) => Completion::succeeded(value),
            Ok(None) => Completion::executor_uncertain(),
            Err(error) => Completion::from_mtproto(&error),
        };
        self.profile
            .journal
            .complete_fenced(flight, completion)
            .await
            .map_err(|_| uncertain())?;
        self.cached(auth, &scope)
            .await
            .map_err(|e| e.with_outcome(EffectOutcome::Unknown))
    }
    async fn cached(&self, auth: &Authorization, expected: &Scope) -> Result<ExecutionResult> {
        let (operation, scope) = self
            .profile
            .journal
            .get_mtproto_scoped(
                &self.profile.name,
                auth.principal.as_str(),
                auth.request.operation_id().expect("write key"),
            )
            .await
            .map_err(storage_error)?
            .ok_or_else(uncertain)?;
        if scope != *expected {
            return Err(denied());
        }
        auth.checked(now()?)?;
        output::project(operation, &scope)
    }
}
enum Input {
    Text(tg_mtproto::Peer, Option<tg_mtproto::SendText>),
    Callback(tg_mtproto::CallbackAnswer),
    Document(tg_mtproto::Peer, tg_mtproto::SendDocument),
}
fn message_value(
    receipt: tg_mtproto::MessageReceipt,
    peer: Option<PeerId>,
    expected: Option<i64>,
) -> Option<serde_json::Value> {
    let peer = peer?;
    if receipt.peer() != peer.as_client_id()
        || receipt.message_id() <= 0
        || expected.is_some_and(|id| id != i64::from(receipt.message_id()))
    {
        return None;
    }
    Some(serde_json::json!({"peer":peer,"message_id":receipt.message_id().to_string()}))
}
fn rpc_deadline(
    flight: &journal::InFlight,
    receipt_margin: Duration,
    auth: &Authorization,
) -> Result<Instant> {
    // Anchor before UTC and policy lock waits; neither can extend this cap.
    let sampled = Instant::now();
    let time = now()?;
    let remaining = flight
        .lease_until_ms()
        .checked_sub(time)
        .and_then(|v| u64::try_from(v).ok())
        .map(Duration::from_millis)
        .ok_or_else(capacity)?;
    let deadline = sampled
        .checked_add(remaining)
        .and_then(|d| d.checked_sub(receipt_margin))
        .ok_or_else(capacity)?;
    let deadline = if let Some(file) = &auth.file {
        files::cap(
            file,
            sampled,
            time,
            deadline.saturating_duration_since(sampled),
        )?
    } else {
        deadline
    };
    auth.write_checked(now()?)?;
    let deadline = auth
        .reference
        .as_ref()
        .map_or(deadline, |r| r.deadline.cap(deadline));
    if deadline <= Instant::now() {
        return Err(capacity());
    }
    Ok(deadline)
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;
