use super::{Client, ClientErrorKind, Outcome, Result, encode_request, error, tl};
use crate::{AccountId, AccountIdentity, AccountKind, Session};
use grammers_session::types::{PeerAuth, PeerInfo};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};
mod interactive;
mod srp;
pub use interactive::{
    AuthStep, CodeChallenge, CodeDelivery, LoginCode, Password, PasswordChallenge,
    PasswordPromptReason, PhoneNumber, UnsupportedAuthFlow, UserLoginTarget,
};

fn advance_epoch(epoch: &std::sync::atomic::AtomicU64) -> Result<u64> {
    epoch
        .fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |value| value.checked_add(1),
        )
        .map(|old| old + 1)
        .map_err(|_| error(ClientErrorKind::Capacity, Outcome::NotSent))
}

/// Application credentials supplied by the trusted host, never by agent tools.
pub struct AppCredentials {
    api_id: i32,
    api_hash: Zeroizing<String>,
}
impl AppCredentials {
    pub fn new(api_id: i32, api_hash: impl Into<String>) -> Result<Self> {
        let api_hash = Zeroizing::new(api_hash.into());
        if api_id <= 0 || api_hash.len() != 32 || !api_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(Self { api_id, api_hash })
    }
}
impl fmt::Debug for AppCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AppCredentials([REDACTED])")
    }
}

/// A private bot login capability. No plaintext getter or serialization is exposed.
pub struct BotToken {
    id: AccountId,
    secret: Zeroizing<String>,
}
impl BotToken {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let secret = Zeroizing::new(token.into());
        let invalid = || error(ClientErrorKind::InvalidInput, Outcome::NotSent);
        if secret.len() > 512 {
            return Err(invalid());
        }
        let (id, value) = secret.split_once(':').ok_or_else(invalid)?;
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit())
            || value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid());
        }
        let id = AccountId::new(id.parse().map_err(|_| invalid())?).map_err(|_| invalid())?;
        Ok(Self { id, secret })
    }
}
impl fmt::Debug for BotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BotToken([REDACTED])")
    }
}

struct BotRequest(tl::functions::auth::ImportBotAuthorization);
impl Drop for BotRequest {
    fn drop(&mut self) {
        self.0.api_hash.zeroize();
        self.0.bot_auth_token.zeroize();
    }
}

pub(super) fn check_stored(session: &Session, expected: AccountIdentity) -> Result<()> {
    let stored = session
        .remembered_account()
        .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?;
    if stored.is_some_and(|stored| stored != expected) {
        return Err(error(ClientErrorKind::IdentityMismatch, Outcome::NotSent));
    }
    Ok(())
}

fn checked_peer(user: tl::enums::User, expected: AccountIdentity) -> Result<PeerInfo> {
    let mismatch = || error(ClientErrorKind::IdentityMismatch, Outcome::Unknown);
    let tl::enums::User::User(user) = user else {
        return Err(mismatch());
    };
    if !user.is_self
        || user.min
        || user.deleted
        || user.id != expected.id.get()
        || user.bot != (expected.kind == AccountKind::Bot)
    {
        return Err(mismatch());
    }
    Ok(PeerInfo::User {
        id: user.id,
        auth: user.access_hash.map(PeerAuth::from_hash),
        bot: Some(user.bot),
        is_self: Some(true),
    })
}

pub(super) async fn verify_in_context(
    context: &super::runtime::Context,
    session: &Session,
    expected: AccountIdentity,
    limit: usize,
) -> Result<AccountIdentity> {
    check_stored(session, expected)?;
    let request = tl::functions::users::GetUsers {
        id: vec![tl::enums::InputUser::UserSelf],
    };
    let mut bytes = encode_request(&request, limit)?;
    let mut users = context
        .invoke::<tl::functions::users::GetUsers>(std::mem::take(&mut *bytes))
        .await?;
    if users.len() != 1 {
        return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
    }
    let peer = checked_peer(users.remove(0), expected)?;
    session
        .remember_verified(expected, peer)
        .await
        .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
    Ok(expected)
}

impl Client {
    /// Verify live self identity and atomically remember it before returning.
    /// Remembered metadata alone is never treated as current authorization.
    pub async fn verify_account(&self, expected: AccountIdentity) -> Result<AccountIdentity> {
        let reservation = self.reserve()?;
        check_stored(&self.inner.session, expected)?;
        let session = self.inner.session.clone();
        let authorization = self.inner.authorization.clone();
        let limit = self.inner.options.max_request_bytes;
        self.submit(reservation, move |context| async move {
            context.hold_authorization_read(authorization).await;
            verify_in_context(&context, &session, expected, limit).await
        })
        .await
    }

    /// Log in as the independently expected bot. No ambiguous login is replayed.
    pub async fn sign_in_bot(
        &self,
        credentials: AppCredentials,
        token: BotToken,
        expected: AccountIdentity,
    ) -> Result<AccountIdentity> {
        if credentials.api_id != self.inner.session.binding().api_id()
            || expected.kind != AccountKind::Bot
            || token.id != expected.id
        {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let reservation = self.reserve()?;
        check_stored(&self.inner.session, expected)?;
        let session = self.inner.session.clone();
        let authorization = self.inner.authorization.clone();
        let epoch = self.inner.auth_epoch.clone();
        let limit = self.inner.options.max_request_bytes;
        self.submit(reservation, move |context| async move {
            context.hold_authorization_write(authorization).await;
            check_stored(&session, expected)?;
            advance_epoch(&epoch)?;
            let request = BotRequest(tl::functions::auth::ImportBotAuthorization {
                flags: 0,
                api_id: credentials.api_id,
                api_hash: credentials.api_hash.to_string(),
                bot_auth_token: token.secret.to_string(),
            });
            let encoded = encode_request(&request.0, limit);
            drop(request);
            drop(credentials);
            drop(token);
            let bytes = encoded?;
            let response = context
                .invoke_auth::<tl::functions::auth::ImportBotAuthorization>(&bytes)
                .await?;
            let tl::enums::auth::Authorization::Authorization(mut auth) = response else {
                return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
            };
            if let Some(token) = auth.future_auth_token.as_mut() {
                token.zeroize();
            }
            let peer = checked_peer(auth.user, expected)?;
            session
                .remember_verified(expected, peer)
                .await
                .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
            Ok(expected)
        })
        .await
    }

    /// Verify the expected account, quarantine before LogOut, then join cleanup.
    /// A failed/uncertain logout preserves quarantine for explicit host recovery.
    pub async fn sign_out(&self, expected: AccountIdentity) -> Result<()> {
        let reservation = self.reserve()?;
        check_stored(&self.inner.session, expected)?;
        let session = self.inner.session.clone();
        let authorization = self.inner.authorization.clone();
        let epoch = self.inner.auth_epoch.clone();
        let limit = self.inner.options.max_request_bytes;
        let result: Result<()> = self
            .submit(reservation, move |context| async move {
                context.hold_authorization_write(authorization).await;
                verify_in_context(&context, &session, expected, limit).await?;
                advance_epoch(&epoch)?;
                context.stop_after_action();
                session
                    .quarantine_authorization()
                    .await
                    .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
                let mut bytes = encode_request(&tl::functions::auth::LogOut {}, limit)?;
                let tl::enums::auth::LoggedOut::Out(mut response) =
                    context.invoke_logout(std::mem::take(&mut *bytes)).await?;
                if let Some(token) = response.future_auth_token.as_mut() {
                    token.zeroize();
                }
                context.confirm_logout();
                Ok(())
            })
            .await;
        result?;
        self.close().await
    }
}
