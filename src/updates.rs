use crate::{Bot, Error, ErrorKind, RequestFuture, Result, Update, User, WebhookInfo};
use serde::Serialize;
use std::future::IntoFuture;

impl Bot {
    /// Read the bot's identity. Does not change webhook or polling configuration.
    pub async fn get_me(&self) -> Result<User> {
        self.perform(async { Ok(self.post("getMe")?.json(&serde_json::json!({}))) })
            .await
    }

    pub async fn get_webhook_info(&self) -> Result<WebhookInfo> {
        self.perform(async { Ok(self.post("getWebhookInfo")?.json(&serde_json::json!({}))) })
            .await
    }

    /// Fetch updates manually. The caller owns offset acknowledgement and poller coordination.
    pub fn get_updates(&self) -> GetUpdates<'_> {
        GetUpdates {
            bot: self,
            params: Params::default(),
        }
    }
}

#[must_use = "requests are lazy; await the request to execute it"]
pub struct GetUpdates<'a> {
    bot: &'a Bot,
    params: Params,
}

#[derive(Default, Serialize)]
struct Params {
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_updates: Option<Vec<String>>,
}

impl GetUpdates<'_> {
    /// Acknowledge all earlier updates. Negative (destructive) offsets are rejected.
    pub fn offset(mut self, offset: i64) -> Self {
        self.params.offset = Some(offset);
        self
    }
    pub fn limit(mut self, limit: u8) -> Self {
        self.params.limit = Some(limit);
        self
    }
    /// Server long-poll timeout in seconds; distinct from the client's overall deadline.
    pub fn timeout(mut self, seconds: u32) -> Self {
        self.params.timeout = Some(seconds);
        self
    }
    pub fn allowed_updates(mut self, kinds: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.params.allowed_updates = Some(kinds.into_iter().map(Into::into).collect());
        self
    }
}

impl<'a> IntoFuture for GetUpdates<'a> {
    type Output = Result<Vec<Update>>;
    type IntoFuture = RequestFuture<'a, Vec<Update>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if self.params.offset.is_some_and(|offset| offset < 0) {
                return Err(Error::local(ErrorKind::Validation, "offset"));
            }
            if self
                .params
                .limit
                .is_some_and(|limit| !(1..=100).contains(&limit))
            {
                return Err(Error::local(ErrorKind::Validation, "limit"));
            }
            self.bot
                .perform(async { Ok(self.bot.post("getUpdates")?.json(&self.params)) })
                .await
        })
    }
}
