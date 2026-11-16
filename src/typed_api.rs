use crate::{Bot, RequestFuture};
use serde::{Serialize, de::DeserializeOwned};
use std::{fmt, future::IntoFuture};

/// Conservative external-effect metadata. This never grants agent permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    Read,
    Write,
    Acknowledge,
    CredentialRead,
    CredentialWrite,
    SessionChange,
}

/// Advice for an explicit retry policy; requests never retry automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySafety {
    ReadOnly,
    Never,
}

impl Effect {
    pub const fn replay_safety(self) -> ReplaySafety {
        match self {
            Self::Read => ReplaySafety::ReadOnly,
            _ => ReplaySafety::Never,
        }
    }
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A generated Bot API method. Custom raw methods cannot implement this trait.
pub trait Method: sealed::Sealed + Serialize + Send + Sync + 'static {
    type Response: DeserializeOwned + Send + 'static;
    const NAME: &'static str;
    const EFFECT: Effect;
    const REPLAY: ReplaySafety = Self::EFFECT.replay_safety();
}

/// The complete pinned Bot API. High-level helpers remain on Bot itself.
#[derive(Clone, Copy)]
pub struct Api<'a> {
    pub(crate) bot: &'a Bot,
}

#[must_use = "requests are lazy; await the request to execute it"]
pub struct Call<'a, M: Method> {
    pub(crate) bot: &'a Bot,
    pub(crate) params: M,
}

impl<M: Method> fmt::Debug for Call<'_, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("method", &M::NAME)
            .finish_non_exhaustive()
    }
}

impl Bot {
    pub fn api(&self) -> Api<'_> {
        Api { bot: self }
    }

    pub fn call<M: Method>(&self, params: M) -> Call<'_, M> {
        Call { bot: self, params }
    }
}

impl<'a, M: Method> IntoFuture for Call<'a, M> {
    type Output = crate::Result<M::Response>;
    type IntoFuture = RequestFuture<'a, M::Response>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.bot
                .perform(async { Ok(self.bot.post(M::NAME)?.json(&self.params)) })
                .await
        })
    }
}
