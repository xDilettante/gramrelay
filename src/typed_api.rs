use crate::{Bot, Error, ErrorKind, InputFile, RequestFuture};
use reqwest::multipart::Form;
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeSet, fmt, future::IntoFuture};

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
    fn attachment_references(&self) -> BTreeSet<String>;
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
    attachments: Vec<(String, InputFile)>,
}

impl<M: Method> Call<'_, M> {
    /// Bind a new file to a typed `attach://name` reference. Preparation is lazy.
    pub fn attach(mut self, name: impl Into<String>, file: InputFile) -> Self {
        self.attachments.push((name.into(), file));
        self
    }
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
        Call {
            bot: self,
            params,
            attachments: Vec::new(),
        }
    }
}

impl<'a, M: Method> IntoFuture for Call<'a, M> {
    type Output = crate::Result<M::Response>;
    type IntoFuture = RequestFuture<'a, M::Response>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.bot
                .perform(async {
                    let params = serde_json::to_value(&self.params)
                        .map_err(|_| Error::local(ErrorKind::Validation, "parameters"))?;
                    let references = self.params.attachment_references();
                    validate_bindings(
                        &params,
                        &references,
                        &self.attachments,
                        self.bot.attachment_limit(),
                    )?;
                    if self.attachments.is_empty() {
                        return Ok(self.bot.post(M::NAME)?.json(&params));
                    }
                    let mut form = Form::new();
                    let object = params
                        .as_object()
                        .ok_or_else(|| Error::local(ErrorKind::Validation, "parameters"))?;
                    for (name, value) in object {
                        let value = value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string());
                        form = form.text(name.clone(), value);
                    }
                    let mut remaining = self.bot.upload_limit();
                    for (name, file) in self.attachments {
                        let (part, length) = file.into_part(remaining).await?;
                        remaining -= length;
                        form = form.part(name, part);
                    }
                    Ok(self.bot.post(M::NAME)?.multipart(form))
                })
                .await
        })
    }
}

fn validate_bindings(
    params: &serde_json::Value,
    references: &BTreeSet<String>,
    files: &[(String, InputFile)],
    limit: usize,
) -> crate::Result<()> {
    if files.len() > limit {
        return Err(Error::local(ErrorKind::Capacity, "attachments"));
    }
    let mut names = BTreeSet::new();
    for (name, _) in files {
        if name.is_empty()
            || name.len() > 64
            || !name.as_bytes()[0].is_ascii_alphabetic()
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || !names.insert(name.clone())
            || params.get(name).is_some()
        {
            return Err(Error::local(ErrorKind::Validation, "attachment_name"));
        }
    }
    if &names != references {
        return Err(Error::local(ErrorKind::Validation, "attachments"));
    }
    Ok(())
}

pub(crate) trait CollectAttachments {
    fn collect_attachments(&self, references: &mut BTreeSet<String>);
}
impl CollectAttachments for String {
    fn collect_attachments(&self, references: &mut BTreeSet<String>) {
        if let Some(name) = self.strip_prefix("attach://") {
            references.insert(name.to_owned());
        }
    }
}
impl<T: CollectAttachments> CollectAttachments for Option<T> {
    fn collect_attachments(&self, references: &mut BTreeSet<String>) {
        if let Some(value) = self {
            value.collect_attachments(references);
        }
    }
}
impl<T: CollectAttachments> CollectAttachments for Box<T> {
    fn collect_attachments(&self, references: &mut BTreeSet<String>) {
        (**self).collect_attachments(references);
    }
}
impl<T: CollectAttachments> CollectAttachments for Vec<T> {
    fn collect_attachments(&self, references: &mut BTreeSet<String>) {
        for value in self {
            value.collect_attachments(references);
        }
    }
}
