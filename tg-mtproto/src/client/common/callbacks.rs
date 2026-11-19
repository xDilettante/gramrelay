use super::*;
use zeroize::{Zeroize, Zeroizing};

/// Trusted Rust intent, not a callback observation or an agent execution grant.
/// Query IDs are opaque 64-bit values, including zero; Debug never exposes them.
pub struct CallbackAnswer {
    query_id: i64,
    text: Option<Zeroizing<String>>,
    alert: bool,
}
impl CallbackAnswer {
    pub fn new(query_id: i64) -> Self {
        Self {
            query_id,
            text: None,
            alert: false,
        }
    }
    pub fn text(mut self, text: impl Into<String>) -> Result<Self> {
        let text = Zeroizing::new(text.into());
        if text.len() > 800 || text.encode_utf16().count() > 200 || text.contains('\0') {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        self.text = Some(text);
        Ok(self)
    }
    pub fn show_alert(mut self, alert: bool) -> Self {
        self.alert = alert;
        self
    }
}
impl fmt::Debug for CallbackAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallbackAnswer([REDACTED])")
    }
}
impl Drop for CallbackAnswer {
    fn drop(&mut self) {
        self.query_id.zeroize();
    }
}
struct Request(tl::functions::messages::SetBotCallbackAnswer);
impl Drop for Request {
    fn drop(&mut self) {
        self.0.query_id.zeroize();
        if let Some(text) = &mut self.0.message {
            text.zeroize();
        }
    }
}
impl Account {
    /// Answer one bot callback without URL/cache options or automatic replay.
    pub async fn answer_callback(&self, intent: CallbackAnswer) -> Result<()> {
        self.answer_callback_inner(intent, None).await
    }
    /// An absolute cap covers queue/identity waits and the target RPC together.
    pub async fn answer_callback_until(
        &self,
        intent: CallbackAnswer,
        deadline: std::time::Instant,
    ) -> Result<()> {
        self.answer_callback_inner(intent, Some(deadline)).await
    }
    async fn answer_callback_inner(
        &self,
        intent: CallbackAnswer,
        deadline: Option<std::time::Instant>,
    ) -> Result<()> {
        if self.expected.kind != crate::AccountKind::Bot {
            return Err(error(
                ClientErrorKind::UnsupportedOperation,
                Outcome::NotSent,
            ));
        }
        let limit = self.client.inner.options.max_request_bytes;
        self.run_until(deadline, move |context, _| async move {
            let mut bytes = {
                let request = Request(tl::functions::messages::SetBotCallbackAnswer {
                    alert: intent.alert,
                    query_id: intent.query_id,
                    message: intent.text.as_ref().map(|t| t.to_string()),
                    url: None,
                    cache_time: 0,
                });
                super::super::encode_request(&request.0, limit)?
            };
            if context
                .invoke::<tl::functions::messages::SetBotCallbackAnswer>(std::mem::take(
                    &mut *bytes,
                ))
                .await?
            {
                Ok(())
            } else {
                Err(error(ClientErrorKind::Decode, Outcome::Unknown))
            }
        })
        .await
    }
}
