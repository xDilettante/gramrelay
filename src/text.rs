use crate::{
    Error, ErrorKind, Result,
    api::types::{MessageEntity, User},
};
use serde::Serialize;
use std::{
    cmp::Reverse,
    fmt,
    ops::{Bound, RangeBounds},
};

/// Explicit server-side markup parsing. Plain text is the default.
#[derive(Debug, Clone, Copy, Serialize)]
pub enum ParseMode {
    #[serde(rename = "HTML")]
    Html,
    MarkdownV2,
    Markdown,
}

/// Every entity kind in the pinned Bot API. Metadata values are not logged.
#[derive(Clone)]
#[non_exhaustive]
pub enum EntityKind {
    Mention,
    Hashtag,
    Cashtag,
    BotCommand,
    Url,
    Email,
    PhoneNumber,
    BankCardNumber,
    TonAddress,
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    Blockquote,
    ExpandableBlockquote,
    Code,
    Pre { language: Option<String> },
    TextLink(String),
    TextMention(Box<User>),
    CustomEmoji(String),
    DateTime { unix_time: i64, format: String },
}

impl EntityKind {
    fn name(&self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Hashtag => "hashtag",
            Self::Cashtag => "cashtag",
            Self::BotCommand => "bot_command",
            Self::Url => "url",
            Self::Email => "email",
            Self::PhoneNumber => "phone_number",
            Self::BankCardNumber => "bank_card_number",
            Self::TonAddress => "ton_address",
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Underline => "underline",
            Self::Strikethrough => "strikethrough",
            Self::Spoiler => "spoiler",
            Self::Blockquote => "blockquote",
            Self::ExpandableBlockquote => "expandable_blockquote",
            Self::Code => "code",
            Self::Pre { .. } => "pre",
            Self::TextLink(_) => "text_link",
            Self::TextMention(_) => "text_mention",
            Self::CustomEmoji(_) => "custom_emoji",
            Self::DateTime { .. } => "date_time",
        }
    }

    fn into_entity(self, offset: i64, length: i64) -> MessageEntity {
        let mut entity = MessageEntity::new(self.name().into(), offset, length);
        match self {
            Self::Pre { language } => entity.language = language,
            Self::TextLink(url) => entity.url = Some(url),
            Self::TextMention(user) => entity.user = Some(user),
            Self::CustomEmoji(id) => entity.custom_emoji_id = Some(id),
            Self::DateTime { unix_time, format } => {
                entity.unix_time = Some(unix_time);
                entity.date_time_format = Some(format);
            }
            _ => {}
        }
        entity
    }
}

impl fmt::Debug for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EntityKind")
            .field(&self.name())
            .finish_non_exhaustive()
    }
}

/// Text and explicit entities, with UTF-16 offsets computed from Rust byte ranges.
/// This differs from the rich-message tree named api::types::RichText.
///
/// ```
/// use tg::{EntityKind, RichText};
/// let text = RichText::new("Привет, ")
///     .append(RichText::styled("🦀", EntityKind::Bold)?);
/// assert_eq!(text.entities()[0].offset, 8);
/// assert_eq!(text.entities()[0].length, 2);
/// # Ok::<(), tg::Error>(())
/// ```
#[derive(Clone)]
pub struct RichText {
    text: String,
    entities: Vec<MessageEntity>,
}

/// Alias for [`RichText`] clarifying that it models `MessageEntity`-based text rather than
/// Bot API 10.1+ [`crate::api::types::RichText`] trees.
pub type EntityText = RichText;

impl fmt::Debug for RichText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RichText")
            .field("bytes", &self.text.len())
            .field("entities", &self.entities.len())
            .finish_non_exhaustive()
    }
}

impl RichText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            entities: Vec::new(),
        }
    }

    pub fn styled(text: impl Into<String>, kind: EntityKind) -> Result<Self> {
        Self::new(text).entity(.., kind)
    }

    pub fn try_from_entities(
        text: impl Into<String>,
        mut entities: Vec<MessageEntity>,
    ) -> Result<Self> {
        let text = text.into();
        validate_entities(&text, &mut entities)?;
        Ok(Self { text, entities })
    }

    /// Ranges use UTF-8 byte indices, like a Rust string slice; offsets sent to Telegram use UTF-16.
    pub fn entity(self, range: impl RangeBounds<usize>, kind: EntityKind) -> Result<Self> {
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(&value) => value,
            Bound::Excluded(&value) => value
                .checked_add(1)
                .ok_or_else(|| invalid("entity_range"))?,
        };
        let end = match range.end_bound() {
            Bound::Unbounded => self.text.len(),
            Bound::Excluded(&value) => value,
            Bound::Included(&value) => value
                .checked_add(1)
                .ok_or_else(|| invalid("entity_range"))?,
        };
        let selected = self
            .text
            .get(start..end)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("entity_range"))?;
        let offset = i64::try_from(self.text[..start].encode_utf16().count())
            .map_err(|_| invalid("entity_range"))?;
        let length =
            i64::try_from(selected.encode_utf16().count()).map_err(|_| invalid("entity_range"))?;
        let mut entities = self.entities;
        entities.push(kind.into_entity(offset, length));
        Self::try_from_entities(self.text, entities)
    }

    pub fn append(mut self, other: impl Into<Self>) -> Self {
        let mut other = other.into();
        // ponytail: append rescans the prefix; cache its length if message-composition benchmarks warrant it.
        // An allocated UTF-8 String has at least as many bytes as UTF-16 units.
        let offset = self.text.encode_utf16().count() as i64;
        for entity in &mut other.entities {
            entity.offset += offset;
        }
        self.text.push_str(&other.text);
        self.entities.extend(other.entities);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn entities(&self) -> &[MessageEntity] {
        &self.entities
    }
    pub fn into_parts(self) -> (String, Vec<MessageEntity>) {
        (self.text, self.entities)
    }
}

impl From<String> for RichText {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}
impl From<&str> for RichText {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}
impl From<&String> for RichText {
    fn from(value: &String) -> Self {
        Self::new(value)
    }
}

const KINDS: &[&str] = &[
    "mention",
    "hashtag",
    "cashtag",
    "bot_command",
    "url",
    "email",
    "phone_number",
    "bank_card_number",
    "ton_address",
    "bold",
    "italic",
    "underline",
    "strikethrough",
    "spoiler",
    "blockquote",
    "expandable_blockquote",
    "code",
    "pre",
    "text_link",
    "text_mention",
    "custom_emoji",
    "date_time",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Style,
    Quote,
    Code,
    Continuous,
}
fn category(kind: &str) -> Category {
    match kind {
        "bold" | "italic" | "underline" | "strikethrough" | "spoiler" => Category::Style,
        "blockquote" | "expandable_blockquote" => Category::Quote,
        "code" | "pre" | "date_time" => Category::Code,
        _ => Category::Continuous,
    }
}

fn invalid(field: &'static str) -> Error {
    Error::local(ErrorKind::Validation, field)
}

fn validate_entities(text: &str, entities: &mut [MessageEntity]) -> Result<()> {
    let mut boundaries = vec![0i64];
    for c in text.chars() {
        boundaries.push(boundaries.last().unwrap() + c.len_utf16() as i64);
    }
    for entity in entities.iter() {
        if !KINDS.contains(&entity.kind.as_str()) {
            return Err(invalid("entity_type"));
        }
        let end = entity
            .offset
            .checked_add(entity.length)
            .ok_or_else(|| invalid("entity_range"))?;
        if entity.offset < 0
            || entity.length <= 0
            || boundaries.binary_search(&entity.offset).is_err()
            || boundaries.binary_search(&end).is_err()
        {
            return Err(invalid("entity_range"));
        }
        validate_metadata(entity)?;
    }
    entities.sort_by_key(|entity| {
        (
            entity.offset,
            Reverse(entity.length),
            match category(&entity.kind) {
                Category::Quote => 0,
                Category::Code => 1,
                Category::Continuous => 2,
                Category::Style => 3,
            },
        )
    });
    let mut stack: Vec<(&MessageEntity, u32)> = Vec::new();
    let mut mask = 0u32;
    let mut quotes = 0;
    let mut continuous = 0;
    for entity in entities.iter() {
        while let Some((parent, bit)) = stack.last() {
            if parent.offset + parent.length > entity.offset {
                break;
            }
            mask &= !bit;
            match category(&parent.kind) {
                Category::Quote => quotes -= 1,
                Category::Continuous => continuous -= 1,
                _ => {}
            }
            stack.pop();
        }
        let class = category(&entity.kind);
        let bit = 1 << KINDS.iter().position(|kind| *kind == entity.kind).unwrap();
        if let Some((parent, _)) = stack.last() {
            // Based on Telegram's containment rules and TDLib's quote/code exception.
            // https://core.telegram.org/bots/api#formatting-options
            // https://github.com/tdlib/td/blob/master/td/telegram/MessageEntity.cpp
            if entity.offset + entity.length > parent.offset + parent.length
                || mask & bit != 0
                || category(&parent.kind) == Category::Code
                || (class == Category::Code && stack.len() != quotes)
                || (matches!(class, Category::Quote | Category::Continuous) && continuous != 0)
                || (class == Category::Quote && quotes != 0)
            {
                return Err(invalid("entities_overlap"));
            }
        }
        match class {
            Category::Quote => quotes += 1,
            Category::Continuous => continuous += 1,
            _ => {}
        }
        mask |= bit;
        stack.push((entity, bit));
    }
    Ok(())
}

fn validate_metadata(entity: &MessageEntity) -> Result<()> {
    let kind = entity.kind.as_str();
    if !entity.unknown_fields().is_empty()
        || (entity.url.is_some() && kind != "text_link")
        || (entity.user.is_some() && kind != "text_mention")
        || (entity.language.is_some() && kind != "pre")
        || (entity.custom_emoji_id.is_some() && kind != "custom_emoji")
        || ((entity.unix_time.is_some() || entity.date_time_format.is_some())
            && kind != "date_time")
    {
        return Err(invalid("entity_metadata"));
    }
    match kind {
        "text_link" => {
            let url = entity
                .url
                .as_deref()
                .filter(|url| !url.is_empty() && !url.chars().any(char::is_control))
                .ok_or_else(|| invalid("entity_url"))?;
            reqwest::Url::parse(url).map_err(|_| invalid("entity_url"))?;
        }
        "text_mention"
            if entity
                .user
                .as_ref()
                .is_none_or(|user| i64::from(user.id) <= 0) =>
        {
            return Err(invalid("entity_user"));
        }
        "custom_emoji" if entity.custom_emoji_id.as_ref().is_none_or(String::is_empty) => {
            return Err(invalid("custom_emoji_id"));
        }
        "date_time" => {
            if entity.unix_time.is_none() {
                return Err(invalid("unix_time"));
            }
            if let Some(format) = &entity.date_time_format
                && !is_valid_date_time_format(format)
            {
                return Err(invalid("date_time_format"));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Validate Telegram's `date_time_format` grammar (`r` or `w?[dD]?[tT]?`).
pub(crate) fn is_valid_date_time_format(format: &str) -> bool {
    let mut rest = format;
    if rest == "r" {
        return true;
    }
    rest = rest.strip_prefix('w').unwrap_or(rest);
    rest = rest.strip_prefix(['d', 'D']).unwrap_or(rest);
    rest = rest.strip_prefix(['t', 'T']).unwrap_or(rest);
    rest.is_empty()
}
