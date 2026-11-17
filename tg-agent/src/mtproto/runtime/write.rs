use super::*;
impl Worker {
    pub(super) async fn write(&self, auth: &Authorization) -> Result<ExecutionResult> {
        let request = &auth.request;
        let peer = request.call().peer().expect("supported text peer");
        let scope = Scope {
            account: self.profile.expected,
            action: request.action(),
            peer: Some(peer),
            file: None,
            reference_hash: None,
        };
        let payload = serde_json::to_value(request).map_err(|_| invalid_policy())?;
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
            auth.checked(now()?)?;
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
                .cached_peer(peer.as_client_id())
                .await
                .map_err(remote)?
                .ok_or_else(|| ExecutorError::new(ExecutorErrorKind::NotFound))?;
            Ok((resolved, intent))
        }
        .await;
        let (resolved, intent) = match preparation {
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
            .begin_mtproto(prepared, scope.clone(), move |time| admission.budget(time))
            .await
        {
            Ok(flight) => flight,
            Err(error) => {
                let _ = self.profile.journal.release_prepared(proof).await;
                return Err(storage_error(error));
            }
        };
        let dispatch = rpc_deadline(&flight, self.receipt_margin, auth);
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
        let response = match request.call() {
            Call::SendMessage(_) => {
                self.profile
                    .account
                    .send_text_until(&resolved, intent.expect("send intent"), deadline)
                    .await
            }
            Call::EditMessageText(args) => {
                self.profile
                    .account
                    .edit_text_until(
                        &resolved,
                        args.message_id.get() as i32,
                        args.text.clone(),
                        deadline,
                    )
                    .await
            }
            _ => unreachable!("supported text action"),
        };
        let completion = match response {
            Ok(receipt) => {
                let expected = if let Call::EditMessageText(args) = request.call() {
                    Some(args.message_id.get())
                } else {
                    None
                };
                if receipt.peer() != peer.as_client_id()
                    || receipt.message_id() <= 0
                    || expected.is_some_and(|id| id != i64::from(receipt.message_id()))
                {
                    Completion::executor_uncertain()
                } else {
                    Completion::succeeded(
                        serde_json::json!({"peer":peer,"message_id":receipt.message_id().to_string()}),
                    )
                }
            }
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
fn rpc_deadline(
    flight: &journal::InFlight,
    receipt_margin: Duration,
    auth: &Authorization,
) -> Result<Instant> {
    // Anchor before UTC and policy lock waits; neither can extend this cap.
    let sampled = Instant::now();
    let remaining = flight
        .lease_until_ms()
        .checked_sub(now()?)
        .and_then(|v| u64::try_from(v).ok())
        .map(Duration::from_millis)
        .ok_or_else(capacity)?;
    let deadline = sampled
        .checked_add(remaining)
        .and_then(|d| d.checked_sub(receipt_margin))
        .ok_or_else(capacity)?;
    auth.checked(now()?)?;
    if deadline <= Instant::now() {
        return Err(capacity());
    }
    Ok(deadline)
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;
