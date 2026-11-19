use super::super::{Core, error::RpcClass, runtime::Context};
use super::{
    Client, ClientErrorKind, Outcome, Result, advance_epoch, checked_peer, encode_request, error,
    srp, tl,
};
use crate::{AccountId, AccountIdentity, AccountKind, Session};
use std::{
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub struct PhoneNumber(Zeroizing<String>);
impl PhoneNumber {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = Zeroizing::new(value.into());
        if !(2..=16).contains(&value.len())
            || !value.starts_with('+')
            || !value[1..].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(Self(value))
    }
}
impl fmt::Debug for PhoneNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PhoneNumber([REDACTED])")
    }
}
pub struct LoginCode(Zeroizing<String>);
impl LoginCode {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = Zeroizing::new(value.into());
        if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(Self(value))
    }
}
impl fmt::Debug for LoginCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginCode([REDACTED])")
    }
}
pub struct Password(Zeroizing<Vec<u8>>);
impl Password {
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = Zeroizing::new(value.into());
        if value.is_empty() || value.len() > 1024 {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        Ok(Self(value))
    }
}
impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserLoginTarget {
    Expected(AccountId),
    NewAccount,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodeDelivery {
    App,
    Sms,
    Call,
    SmsWord,
    SmsPhrase,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedAuthFlow {
    SignUp,
    Payment,
    Email,
    Delivery,
    PasswordAlgorithm,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordPromptReason {
    Required,
    InvalidPassword,
    Expired,
}
#[derive(Debug)]
#[non_exhaustive]
pub enum AuthStep {
    Authorized(AccountIdentity),
    CodeRequired(CodeChallenge),
    PasswordRequired(PasswordChallenge),
    Unsupported(UnsupportedAuthFlow),
}

#[derive(Clone)]
struct Binding {
    generation: Weak<Core>,
    epoch: u64,
    expires: Instant,
    expected: Option<AccountIdentity>,
}
impl Binding {
    fn check(&self, generation: &Weak<Core>, epoch: &AtomicU64, session: &Session) -> Result<()> {
        if !Weak::ptr_eq(&self.generation, generation)
            || self.epoch != epoch.load(Ordering::Acquire)
            || Instant::now() >= self.expires
        {
            return Err(error(ClientErrorKind::InvalidChallenge, Outcome::NotSent));
        }
        let stored = session
            .remembered_account()
            .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?;
        if stored.is_some() && stored != self.expected {
            return Err(error(ClientErrorKind::IdentityMismatch, Outcome::NotSent));
        }
        Ok(())
    }
}
pub struct CodeChallenge {
    binding: Binding,
    phone: Arc<Zeroizing<String>>,
    hash: Arc<Zeroizing<String>>,
    delivery: CodeDelivery,
    length: Option<usize>,
    retry: Option<Duration>,
}
impl CodeChallenge {
    pub fn delivery(&self) -> CodeDelivery {
        self.delivery
    }
    pub fn expected_length(&self) -> Option<usize> {
        self.length
    }
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry
    }
    #[cfg(test)]
    pub(crate) fn expire_for_test(&mut self) {
        self.binding.expires = Instant::now() - Duration::from_secs(1);
    }
}
impl fmt::Debug for CodeChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodeChallenge([REDACTED])")
    }
}
pub struct PasswordChallenge {
    binding: Binding,
    parameters: Arc<srp::Parameters>,
    hint: Option<Zeroizing<String>>,
    reason: PasswordPromptReason,
}
impl PasswordChallenge {
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_ref().map(|hint| hint.as_str())
    }
    pub fn reason(&self) -> PasswordPromptReason {
        self.reason
    }
}
impl fmt::Debug for PasswordChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PasswordChallenge([REDACTED])")
    }
}

fn target(session: &Session, target: UserLoginTarget) -> Result<Option<AccountIdentity>> {
    let expected = match target {
        UserLoginTarget::Expected(id) => Some(AccountIdentity {
            id,
            kind: AccountKind::User,
        }),
        UserLoginTarget::NewAccount => None,
    };
    let stored = session
        .remembered_account()
        .map_err(|_| error(ClientErrorKind::Session, Outcome::NotSent))?;
    if stored.is_some() && stored != expected {
        return Err(error(ClientErrorKind::IdentityMismatch, Outcome::NotSent));
    }
    Ok(expected)
}
fn consume(context: &Context, epoch: &AtomicU64) -> Result<u64> {
    advance_epoch(epoch).map_err(|_| {
        context.stop_after_action();
        error(ClientErrorKind::Capacity, Outcome::Unknown)
    })
}
async fn authorized(
    context: &Context,
    session: &Session,
    epoch: &AtomicU64,
    binding: &Binding,
    mut auth: tl::enums::auth::Authorization,
) -> Result<AuthStep> {
    if let tl::enums::auth::Authorization::Authorization(auth) = &mut auth {
        auth.future_auth_token.zeroize();
    }
    consume(context, epoch)?;
    let tl::enums::auth::Authorization::Authorization(auth) = auth else {
        return Ok(AuthStep::Unsupported(UnsupportedAuthFlow::SignUp));
    };
    let tl::enums::User::User(user) = &auth.user else {
        return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
    };
    let identity = AccountIdentity {
        id: AccountId::new(user.id)
            .map_err(|_| error(ClientErrorKind::IdentityMismatch, Outcome::Unknown))?,
        kind: AccountKind::User,
    };
    if binding
        .expected
        .is_some_and(|expected| expected != identity)
    {
        return Err(error(ClientErrorKind::IdentityMismatch, Outcome::Unknown));
    }
    let peer = checked_peer(auth.user, identity)?;
    session
        .remember_verified(identity, peer)
        .await
        .map_err(|_| error(ClientErrorKind::Session, Outcome::Unknown))?;
    Ok(AuthStep::Authorized(identity))
}
async fn password_step(
    context: &Context,
    mut binding: Binding,
    limit: usize,
    reason: PasswordPromptReason,
) -> Result<AuthStep> {
    let mut bytes = encode_request(&tl::functions::account::GetPassword {}, limit)?;
    let tl::enums::account::Password::Password(info) = context
        .invoke_auth::<tl::functions::account::GetPassword>(&bytes)
        .await?;
    bytes.zeroize();
    let Some(info) = srp::parameters(info)? else {
        return Ok(AuthStep::Unsupported(
            UnsupportedAuthFlow::PasswordAlgorithm,
        ));
    };
    binding.expires = Instant::now() + Duration::from_secs(300);
    Ok(AuthStep::PasswordRequired(PasswordChallenge {
        binding,
        parameters: Arc::new(info.parameters),
        hint: info.hint,
        reason,
    }))
}
struct SendCode(tl::functions::auth::SendCode);
impl Drop for SendCode {
    fn drop(&mut self) {
        self.0.api_hash.zeroize();
        self.0.phone_number.zeroize();
    }
}
struct SignIn(tl::functions::auth::SignIn);
impl Drop for SignIn {
    fn drop(&mut self) {
        self.0.phone_number.zeroize();
        self.0.phone_code_hash.zeroize();
        self.0.phone_code.zeroize();
    }
}
struct CheckPassword(tl::functions::auth::CheckPassword);
impl Drop for CheckPassword {
    fn drop(&mut self) {
        if let tl::enums::InputCheckPasswordSrp::Srp(proof) = &mut self.0.password {
            proof.a.zeroize();
            proof.m1.zeroize();
        }
    }
}

impl Client {
    pub async fn begin_user_login(
        &self,
        credentials: super::AppCredentials,
        phone: PhoneNumber,
        login_target: UserLoginTarget,
    ) -> Result<AuthStep> {
        if credentials.api_id != self.inner.session.binding().api_id() {
            return Err(error(ClientErrorKind::InvalidInput, Outcome::NotSent));
        }
        let reservation = self.reserve()?;
        target(&self.inner.session, login_target)?;
        let (session, authorization, epoch, generation, limit) = (
            self.inner.session.clone(),
            self.inner.authorization.clone(),
            self.inner.auth_epoch.clone(),
            Arc::downgrade(&self.inner),
            self.inner.options.max_request_bytes,
        );
        self.submit(reservation, move |context| async move {
            context.hold_authorization_write(authorization).await;
            let expected = target(&session, login_target)?;
            let serial = advance_epoch(&epoch)?;
            let binding = Binding {
                generation,
                epoch: serial,
                expires: Instant::now() + Duration::from_secs(300),
                expected,
            };
            let request = SendCode(tl::functions::auth::SendCode {
                phone_number: phone.0.to_string(),
                api_id: credentials.api_id,
                api_hash: credentials.api_hash.to_string(),
                settings: tl::types::CodeSettings {
                    allow_flashcall: false,
                    current_number: false,
                    allow_app_hash: false,
                    allow_missed_call: false,
                    allow_firebase: false,
                    logout_tokens: None,
                    token: None,
                    app_sandbox: None,
                    unknown_number: false,
                }
                .into(),
            });
            let bytes = encode_request(&request.0, limit);
            drop(request);
            drop(credentials);
            let bytes = bytes?;
            let response = match context
                .invoke_auth::<tl::functions::auth::SendCode>(&bytes)
                .await
            {
                Err(failure) if failure.rpc_class() == RpcClass::PasswordNeeded => {
                    let mut binding = binding;
                    binding.epoch = consume(&context, &epoch)?;
                    return password_step(&context, binding, limit, PasswordPromptReason::Required)
                        .await;
                }
                result => result?,
            };
            match response {
                tl::enums::auth::SentCode::Success(success) => {
                    authorized(&context, &session, &epoch, &binding, success.authorization).await
                }
                tl::enums::auth::SentCode::PaymentRequired(mut payment) => {
                    payment.phone_code_hash.zeroize();
                    Ok(AuthStep::Unsupported(UnsupportedAuthFlow::Payment))
                }
                tl::enums::auth::SentCode::Code(mut code) => {
                    let binding = Binding {
                        expires: Instant::now() + Duration::from_secs(300),
                        ..binding
                    };
                    let (delivery, length) = match code.r#type {
                        tl::enums::auth::SentCodeType::App(value) => {
                            (CodeDelivery::App, Some(value.length))
                        }
                        tl::enums::auth::SentCodeType::Sms(value) => {
                            (CodeDelivery::Sms, Some(value.length))
                        }
                        tl::enums::auth::SentCodeType::Call(value) => {
                            (CodeDelivery::Call, Some(value.length))
                        }
                        tl::enums::auth::SentCodeType::SmsWord(_) => (CodeDelivery::SmsWord, None),
                        tl::enums::auth::SentCodeType::SmsPhrase(_) => {
                            (CodeDelivery::SmsPhrase, None)
                        }
                        tl::enums::auth::SentCodeType::EmailCode(_)
                        | tl::enums::auth::SentCodeType::SetUpEmailRequired(_) => {
                            code.phone_code_hash.zeroize();
                            return Ok(AuthStep::Unsupported(UnsupportedAuthFlow::Email));
                        }
                        _ => {
                            code.phone_code_hash.zeroize();
                            return Ok(AuthStep::Unsupported(UnsupportedAuthFlow::Delivery));
                        }
                    };
                    let hash = Zeroizing::new(std::mem::take(&mut code.phone_code_hash));
                    if hash.is_empty()
                        || hash.len() > 2048
                        || length.is_some_and(|length| !(1..=128).contains(&length))
                    {
                        return Err(error(ClientErrorKind::Decode, Outcome::Unknown));
                    }
                    let retry = code
                        .timeout
                        .and_then(|value| u64::try_from(value).ok())
                        .filter(|value| *value <= 3600)
                        .map(Duration::from_secs);
                    Ok(AuthStep::CodeRequired(CodeChallenge {
                        binding,
                        phone: Arc::new(phone.0),
                        hash: Arc::new(hash),
                        delivery,
                        length: length.map(|value| value as usize),
                        retry,
                    }))
                }
            }
        })
        .await
    }
    pub async fn submit_login_code(
        &self,
        challenge: &CodeChallenge,
        code: LoginCode,
    ) -> Result<AuthStep> {
        let reservation = self.reserve()?;
        let (session, authorization, epoch, generation, limit) = (
            self.inner.session.clone(),
            self.inner.authorization.clone(),
            self.inner.auth_epoch.clone(),
            Arc::downgrade(&self.inner),
            self.inner.options.max_request_bytes,
        );
        challenge.binding.check(&generation, &epoch, &session)?;
        let (binding, phone, hash) = (
            challenge.binding.clone(),
            challenge.phone.clone(),
            challenge.hash.clone(),
        );
        self.submit(reservation, move |context| async move {
            context.hold_authorization_write(authorization).await;
            binding.check(&generation, &epoch, &session)?;
            let request = SignIn(tl::functions::auth::SignIn {
                phone_number: phone.to_string(),
                phone_code_hash: hash.to_string(),
                phone_code: Some(code.0.to_string()),
                email_verification: None,
            });
            let bytes = encode_request(&request.0, limit);
            drop(request);
            drop(code);
            drop(phone);
            drop(hash);
            let bytes = bytes?;
            match context
                .invoke_auth::<tl::functions::auth::SignIn>(&bytes)
                .await
            {
                Ok(auth) => authorized(&context, &session, &epoch, &binding, auth).await,
                Err(failure) if failure.rpc_class() == RpcClass::PasswordNeeded => {
                    let mut binding = binding;
                    binding.epoch = consume(&context, &epoch)?;
                    password_step(&context, binding, limit, PasswordPromptReason::Required).await
                }
                Err(failure) => {
                    if failure.rpc_class() == RpcClass::CodeExpired {
                        consume(&context, &epoch)?;
                    }
                    Err(failure)
                }
            }
        })
        .await
    }
    pub async fn submit_password(
        &self,
        challenge: &PasswordChallenge,
        password: Password,
    ) -> Result<AuthStep> {
        let reservation = self.reserve()?;
        let (session, authorization, epoch, generation, limit) = (
            self.inner.session.clone(),
            self.inner.authorization.clone(),
            self.inner.auth_epoch.clone(),
            Arc::downgrade(&self.inner),
            self.inner.options.max_request_bytes,
        );
        challenge.binding.check(&generation, &epoch, &session)?;
        let (mut binding, parameters) = (challenge.binding.clone(), challenge.parameters.clone());
        self.submit(reservation, move |context| async move {
            context.hold_authorization_write(authorization).await;
            binding.check(&generation, &epoch, &session)?;
            binding.epoch = advance_epoch(&epoch)?;
            let id = parameters.id;
            let proof = context
                .compute(move || srp::proof(&parameters, &password.0))
                .await?;
            let request = CheckPassword(tl::functions::auth::CheckPassword {
                password: tl::enums::InputCheckPasswordSrp::Srp(tl::types::InputCheckPasswordSrp {
                    srp_id: id,
                    a: proof.a.to_vec(),
                    m1: proof.m1.to_vec(),
                }),
            });
            let bytes = encode_request(&request.0, limit);
            drop(request);
            drop(proof);
            let bytes = bytes?;
            match context
                .invoke_auth::<tl::functions::auth::CheckPassword>(&bytes)
                .await
            {
                Ok(auth) => authorized(&context, &session, &epoch, &binding, auth).await,
                Err(failure)
                    if matches!(
                        failure.rpc_class(),
                        RpcClass::PasswordInvalid | RpcClass::SrpExpired
                    ) =>
                {
                    let reason = if failure.rpc_class() == RpcClass::PasswordInvalid {
                        PasswordPromptReason::InvalidPassword
                    } else {
                        PasswordPromptReason::Expired
                    };
                    password_step(&context, binding, limit, reason).await
                }
                Err(failure) => Err(failure),
            }
        })
        .await
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    #[test]
    fn owned_password_allocations_transfer_directly_into_zeroizing_storage() {
        let bytes = vec![7u8; 64];
        let pointer = bytes.as_ptr();
        let password = Password::new(bytes).unwrap();
        assert_eq!(password.0.as_ptr(), pointer);
        let text = "SYNTHETIC_PRIVATE_PASSWORD".repeat(2);
        let pointer = text.as_ptr();
        let password = Password::new(text).unwrap();
        assert_eq!(password.0.as_ptr(), pointer);
    }
}
