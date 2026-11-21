//! Ergonomic builders over generated Bot API 10.1+ [`InputRichMessage`] wire types.
//!
//! The wire types under [`crate::api::types`] remain 1:1 with the pinned Telegram Bot API
//! schema. This module layers typed options, `impl Into<...>` ergonomics, and local validation
//! (such as `date_time_format` grammar checking) on top of those types.

use crate::{
    Error, ErrorKind, Result,
    api::{
        True,
        types::{
            CopyTextButton, InputMediaPhoto, InputRichBlock, InputRichBlockBlockQuotation,
            InputRichBlockButtons, InputRichBlockDetails, InputRichBlockDivider,
            InputRichBlockExpandableBlockQuotation, InputRichBlockFooter, InputRichBlockList,
            InputRichBlockListItem, InputRichBlockMathematicalExpression, InputRichBlockParagraph,
            InputRichBlockPhoto, InputRichBlockPreformatted, InputRichBlockPullQuotation,
            InputRichBlockSectionHeading, InputRichBlockTable, InputRichMessage, RichBlockCaption,
            RichBlockTableCell, RichMessageButton, RichText as WireRichText, RichTextBold,
            RichTextCode, RichTextCustomEmoji, RichTextDateTime, RichTextItalic, RichTextMarked,
            RichTextMathematicalExpression, RichTextSpoiler, RichTextStrikethrough,
            RichTextSubscript, RichTextSuperscript, RichTextUnderline, RichTextUrl,
        },
    },
    text::is_valid_date_time_format,
    typed_api::is_valid_attachment_name,
};

/// Section heading level (`1..=6`) for [`InputRichBlockSectionHeading`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingLevel {
    H1 = 1,
    H2 = 2,
    H3 = 3,
    H4 = 4,
    H5 = 5,
    H6 = 6,
}

impl HeadingLevel {
    pub fn as_i64(self) -> i64 {
        self as i64
    }
}

/// Horizontal alignment for rich table cells and embedded button groups.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }
}

/// Vertical alignment for rich table cells.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VerticalAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

impl VerticalAlign {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Middle => "middle",
            Self::Bottom => "bottom",
        }
    }
}

/// Visual button style (`primary`, `success`, `danger`) supported by Telegram buttons.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ButtonStyle {
    #[default]
    Default,
    Primary,
    Success,
    Danger,
}

impl ButtonStyle {
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Primary => Some("primary"),
            Self::Success => Some("success"),
            Self::Danger => Some("danger"),
        }
    }
}

/// Ergonomic inline rich-text node wrapping [`crate::api::types::RichText`].
#[derive(Debug, Clone)]
pub struct RichSpan(WireRichText);

impl RichSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self(WireRichText::String(text.into()))
    }

    pub fn concat(items: impl IntoIterator<Item = impl Into<Self>>) -> Self {
        let mut nodes: Vec<WireRichText> = items
            .into_iter()
            .map(|item| item.into().into_wire())
            .collect();
        match nodes.len() {
            0 => Self::plain(""),
            1 => Self(nodes.pop().expect("single item")),
            _ => Self(WireRichText::Array(nodes)),
        }
    }

    pub fn bold(text: impl Into<Self>) -> Self {
        Self(RichTextBold::new(text.into().into_wire()).into())
    }

    pub fn italic(text: impl Into<Self>) -> Self {
        Self(RichTextItalic::new(text.into().into_wire()).into())
    }

    pub fn underline(text: impl Into<Self>) -> Self {
        Self(RichTextUnderline::new(text.into().into_wire()).into())
    }

    pub fn strikethrough(text: impl Into<Self>) -> Self {
        Self(RichTextStrikethrough::new(text.into().into_wire()).into())
    }

    pub fn spoiler(text: impl Into<Self>) -> Self {
        Self(RichTextSpoiler::new(text.into().into_wire()).into())
    }

    pub fn marked(text: impl Into<Self>) -> Self {
        Self(RichTextMarked::new(text.into().into_wire()).into())
    }

    pub fn code(text: impl Into<String>) -> Self {
        Self(RichTextCode::new(WireRichText::String(text.into())).into())
    }

    pub fn subscript(text: impl Into<Self>) -> Self {
        Self(RichTextSubscript::new(text.into().into_wire()).into())
    }

    pub fn superscript(text: impl Into<Self>) -> Self {
        Self(RichTextSuperscript::new(text.into().into_wire()).into())
    }

    pub fn url(label: impl Into<Self>, url: impl Into<String>) -> Self {
        Self(RichTextUrl::new(label.into().into_wire(), url.into()).into())
    }

    pub fn custom_emoji(
        custom_emoji_id: impl Into<String>,
        alternative_text: impl Into<String>,
    ) -> Self {
        Self(RichTextCustomEmoji::new(custom_emoji_id.into(), alternative_text.into()).into())
    }

    pub fn math(expression: impl Into<String>) -> Self {
        Self(RichTextMathematicalExpression::new(expression.into()).into())
    }

    /// Build a [`RichTextDateTime`] span after validating Telegram's `date_time_format` grammar
    /// (`r` or `w?[dD]?[tT]?`).
    pub fn date_time(
        fallback: impl Into<Self>,
        unix_time: i64,
        format: impl Into<String>,
    ) -> Result<Self> {
        let format = format.into();
        if !is_valid_date_time_format(&format) {
            return Err(Error::local(ErrorKind::Validation, "date_time_format"));
        }
        Ok(Self(
            RichTextDateTime::new(fallback.into().into_wire(), unix_time, format).into(),
        ))
    }

    pub fn into_wire(self) -> WireRichText {
        self.0
    }
}

impl From<&str> for RichSpan {
    fn from(value: &str) -> Self {
        Self::plain(value)
    }
}

impl From<String> for RichSpan {
    fn from(value: String) -> Self {
        Self::plain(value)
    }
}

impl From<WireRichText> for RichSpan {
    fn from(value: WireRichText) -> Self {
        Self(value)
    }
}

impl From<Vec<RichSpan>> for RichSpan {
    fn from(items: Vec<RichSpan>) -> Self {
        Self::concat(items)
    }
}

impl From<RichSpan> for WireRichText {
    fn from(value: RichSpan) -> Self {
        value.0
    }
}

/// Builder for [`RichMessageButton`] inside an [`InputRichBlockButtons`] block.
#[derive(Debug, Clone)]
pub struct RichButton(RichMessageButton);

impl RichButton {
    pub fn callback(text: impl Into<RichSpan>, callback_data: impl Into<String>) -> Self {
        Self(RichMessageButton::new(text.into().into_wire()).callback_data(callback_data.into()))
    }

    pub fn url(text: impl Into<RichSpan>, url: impl Into<String>) -> Self {
        Self(RichMessageButton::new(text.into().into_wire()).url(url.into()))
    }

    pub fn copy_text(text: impl Into<RichSpan>, copy_value: impl Into<String>) -> Self {
        Self(
            RichMessageButton::new(text.into().into_wire())
                .copy_text(CopyTextButton::new(copy_value.into())),
        )
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.0.style = style.as_str().map(str::to_string);
        self
    }

    pub fn into_wire(self) -> RichMessageButton {
        self.0
    }
}

impl From<RichButton> for RichMessageButton {
    fn from(value: RichButton) -> Self {
        value.0
    }
}

/// Builder for a single [`RichBlockTableCell`].
#[derive(Debug, Clone)]
pub struct RichTableCell {
    text: Option<WireRichText>,
    align: TextAlign,
    valign: VerticalAlign,
    is_header: bool,
    colspan: Option<i64>,
    rowspan: Option<i64>,
}

impl RichTableCell {
    pub fn new(text: impl Into<RichSpan>) -> Self {
        Self {
            text: Some(text.into().into_wire()),
            align: TextAlign::Left,
            valign: VerticalAlign::Middle,
            is_header: false,
            colspan: None,
            rowspan: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            text: None,
            align: TextAlign::Left,
            valign: VerticalAlign::Middle,
            is_header: false,
            colspan: None,
            rowspan: None,
        }
    }

    pub fn header(text: impl Into<RichSpan>) -> Self {
        Self::new(text).is_header(true)
    }

    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    pub fn valign(mut self, valign: VerticalAlign) -> Self {
        self.valign = valign;
        self
    }

    pub fn is_header(mut self, is_header: bool) -> Self {
        self.is_header = is_header;
        self
    }

    pub fn colspan(mut self, colspan: i64) -> Self {
        self.colspan = Some(colspan);
        self
    }

    pub fn rowspan(mut self, rowspan: i64) -> Self {
        self.rowspan = Some(rowspan);
        self
    }

    pub fn into_wire(self) -> RichBlockTableCell {
        let mut cell = RichBlockTableCell::new(
            self.align.as_str().to_string(),
            self.valign.as_str().to_string(),
        );
        if let Some(text) = self.text {
            cell = cell.text(text);
        }
        if self.is_header {
            cell = cell.is_header(True);
        }
        if let Some(colspan) = self.colspan {
            cell = cell.colspan(colspan);
        }
        if let Some(rowspan) = self.rowspan {
            cell = cell.rowspan(rowspan);
        }
        cell
    }
}

impl From<&str> for RichTableCell {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for RichTableCell {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<RichSpan> for RichTableCell {
    fn from(value: RichSpan) -> Self {
        Self::new(value)
    }
}

impl From<RichTableCell> for RichBlockTableCell {
    fn from(value: RichTableCell) -> Self {
        value.into_wire()
    }
}

/// Builder for [`InputRichBlockTable`].
#[derive(Debug, Default, Clone)]
pub struct RichTable {
    rows: Vec<Vec<RichBlockTableCell>>,
    bordered: bool,
    striped: bool,
    compact: bool,
    caption: Option<WireRichText>,
}

impl RichTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn header_row(mut self, cells: impl IntoIterator<Item = impl Into<RichTableCell>>) -> Self {
        let row = cells
            .into_iter()
            .map(|cell| cell.into().is_header(true).into_wire())
            .collect();
        self.rows.push(row);
        self
    }

    pub fn row(mut self, cells: impl IntoIterator<Item = impl Into<RichTableCell>>) -> Self {
        let row = cells
            .into_iter()
            .map(|cell| cell.into().into_wire())
            .collect();
        self.rows.push(row);
        self
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub fn caption(mut self, caption: impl Into<RichSpan>) -> Self {
        self.caption = Some(caption.into().into_wire());
        self
    }

    pub fn build(self) -> InputRichBlockTable {
        let mut table = InputRichBlockTable::new(self.rows);
        if self.bordered {
            table = table.is_bordered(True);
        }
        if self.striped {
            table = table.is_striped(True);
        }
        if self.compact {
            table = table.is_compact(True);
        }
        if let Some(caption) = self.caption {
            table = table.caption(caption);
        }
        table
    }
}

impl From<RichTable> for InputRichBlock {
    fn from(value: RichTable) -> Self {
        value.build().into()
    }
}

/// Builder for [`InputRichBlockListItem`].
#[derive(Debug, Clone)]
pub struct RichListItem(InputRichBlockListItem);

impl RichListItem {
    pub fn text(text: impl Into<RichSpan>) -> Self {
        let paragraph: InputRichBlock =
            InputRichBlockParagraph::new(text.into().into_wire()).into();
        Self(InputRichBlockListItem::new(vec![paragraph]))
    }

    pub fn checkbox(text: impl Into<RichSpan>, checked: bool) -> Self {
        let paragraph: InputRichBlock =
            InputRichBlockParagraph::new(text.into().into_wire()).into();
        let mut item = InputRichBlockListItem::new(vec![paragraph]).has_checkbox(True);
        if checked {
            item = item.is_checked(True);
        }
        Self(item)
    }

    pub fn blocks(blocks: Vec<InputRichBlock>) -> Self {
        Self(InputRichBlockListItem::new(blocks))
    }

    pub fn into_wire(self) -> InputRichBlockListItem {
        self.0
    }
}

impl From<RichListItem> for InputRichBlockListItem {
    fn from(value: RichListItem) -> Self {
        value.0
    }
}

/// Builder for [`InputRichBlockList`].
#[derive(Debug, Default, Clone)]
pub struct RichList {
    items: Vec<InputRichBlockListItem>,
}

impl RichList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(mut self, item: impl Into<InputRichBlockListItem>) -> Self {
        self.items.push(item.into());
        self
    }

    pub fn text(self, text: impl Into<RichSpan>) -> Self {
        self.item(RichListItem::text(text))
    }

    pub fn checkbox(self, text: impl Into<RichSpan>, checked: bool) -> Self {
        self.item(RichListItem::checkbox(text, checked))
    }

    pub fn build(self) -> InputRichBlockList {
        InputRichBlockList::new(self.items)
    }
}

impl From<RichList> for InputRichBlock {
    fn from(value: RichList) -> Self {
        value.build().into()
    }
}

/// High-level document builder producing [`InputRichMessage`] or nested [`InputRichBlock`] lists.
#[derive(Debug, Default, Clone)]
pub struct RichDoc {
    blocks: Vec<InputRichBlock>,
    is_rtl: Option<bool>,
    skip_entity_detection: Option<bool>,
}

impl RichDoc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn heading(mut self, level: HeadingLevel, text: impl Into<RichSpan>) -> Self {
        self.blocks.push(
            InputRichBlockSectionHeading::new(text.into().into_wire(), level.as_i64()).into(),
        );
        self
    }

    pub fn paragraph(mut self, text: impl Into<RichSpan>) -> Self {
        self.blocks
            .push(InputRichBlockParagraph::new(text.into().into_wire()).into());
        self
    }

    pub fn divider(mut self) -> Self {
        self.blocks.push(InputRichBlockDivider::new().into());
        self
    }

    pub fn footer(mut self, text: impl Into<RichSpan>) -> Self {
        self.blocks
            .push(InputRichBlockFooter::new(text.into().into_wire()).into());
        self
    }

    pub fn preformatted(mut self, text: impl Into<RichSpan>, language: Option<&str>) -> Self {
        let mut block = InputRichBlockPreformatted::new(text.into().into_wire());
        if let Some(language) = language {
            block = block.language(language.to_string());
        }
        self.blocks.push(block.into());
        self
    }

    pub fn blockquote(mut self, body: RichDoc, credit: Option<RichSpan>) -> Self {
        let mut block = InputRichBlockBlockQuotation::new(body.blocks);
        if let Some(credit) = credit {
            block = block.credit(credit.into_wire());
        }
        self.blocks.push(block.into());
        self
    }

    pub fn expandable_quote(mut self, text: impl Into<RichSpan>, credit: Option<RichSpan>) -> Self {
        let mut block = InputRichBlockExpandableBlockQuotation::new(text.into().into_wire());
        if let Some(credit) = credit {
            block = block.credit(credit.into_wire());
        }
        self.blocks.push(block.into());
        self
    }

    pub fn pull_quote(mut self, text: impl Into<RichSpan>, credit: Option<RichSpan>) -> Self {
        let mut block = InputRichBlockPullQuotation::new(text.into().into_wire());
        if let Some(credit) = credit {
            block = block.credit(credit.into_wire());
        }
        self.blocks.push(block.into());
        self
    }

    pub fn math_block(mut self, expression: impl Into<String>) -> Self {
        self.blocks
            .push(InputRichBlockMathematicalExpression::new(expression.into()).into());
        self
    }

    pub fn details(mut self, summary: impl Into<RichSpan>, is_open: bool, body: RichDoc) -> Self {
        let mut block = InputRichBlockDetails::new(summary.into().into_wire(), body.blocks);
        if is_open {
            block = block.is_open(True);
        }
        self.blocks.push(block.into());
        self
    }

    pub fn table(mut self, table: RichTable) -> Self {
        self.blocks.push(table.into());
        self
    }

    pub fn list(mut self, list: RichList) -> Self {
        self.blocks.push(list.into());
        self
    }

    pub fn buttons(
        mut self,
        buttons: impl IntoIterator<Item = impl Into<RichMessageButton>>,
        align: Option<TextAlign>,
    ) -> Self {
        let mut block = InputRichBlockButtons::new(buttons.into_iter().map(Into::into).collect());
        if let Some(align) = align {
            block = block.align(align.as_str().to_string());
        }
        self.blocks.push(block.into());
        self
    }

    /// Add a photo block referencing a named multipart attachment (`attach://<attachment_name>`).
    /// Bind the matching [`crate::InputFile`] with `Call::attach(attachment_name, file)`.
    pub fn photo_attach(
        mut self,
        attachment_name: impl Into<String>,
        caption: Option<RichSpan>,
    ) -> Self {
        let media = InputMediaPhoto::new(format!("attach://{}", attachment_name.into()));
        let mut block = InputRichBlockPhoto::new(media);
        if let Some(caption) = caption {
            block = block.caption(RichBlockCaption::new(caption.into_wire()));
        }
        self.blocks.push(block.into());
        self
    }

    pub fn photo(mut self, photo: InputMediaPhoto, caption: Option<RichBlockCaption>) -> Self {
        let mut block = InputRichBlockPhoto::new(photo);
        if let Some(caption) = caption {
            block = block.caption(caption);
        }
        self.blocks.push(block.into());
        self
    }

    /// Escape hatch for appending any generated [`InputRichBlock`] directly.
    pub fn block(mut self, block: impl Into<InputRichBlock>) -> Self {
        self.blocks.push(block.into());
        self
    }

    pub fn is_rtl(mut self, is_rtl: bool) -> Self {
        self.is_rtl = Some(is_rtl);
        self
    }

    pub fn skip_entity_detection(mut self, skip: bool) -> Self {
        self.skip_entity_detection = Some(skip);
        self
    }

    /// Validate all blocks (including nested sections) and return them as a wire block vector.
    pub fn try_into_blocks(self) -> Result<Vec<InputRichBlock>> {
        validate_blocks(&self.blocks)?;
        Ok(self.blocks)
    }

    /// Validate all blocks (including nested sections) and build an [`InputRichMessage`].
    pub fn build(self) -> Result<InputRichMessage> {
        validate_blocks(&self.blocks)?;
        let mut message = InputRichMessage::new().blocks(self.blocks);
        if let Some(is_rtl) = self.is_rtl {
            message = message.is_rtl(is_rtl);
        }
        if let Some(skip) = self.skip_entity_detection {
            message = message.skip_entity_detection(skip);
        }
        Ok(message)
    }
}

impl TryFrom<RichDoc> for InputRichMessage {
    type Error = Error;

    fn try_from(value: RichDoc) -> Result<Self> {
        value.build()
    }
}

fn validate_blocks(blocks: &[InputRichBlock]) -> Result<()> {
    if blocks.is_empty() {
        return Err(Error::local(ErrorKind::Validation, "rich_blocks"));
    }
    for block in blocks {
        validate_block(block)?;
    }
    Ok(())
}

fn validate_block(block: &InputRichBlock) -> Result<()> {
    match block {
        InputRichBlock::InputRichBlockSectionHeading(heading) => {
            if !(1..=6).contains(&heading.size) {
                return Err(Error::local(ErrorKind::Validation, "heading_size"));
            }
            validate_rich_text(&heading.text)
        }
        InputRichBlock::InputRichBlockParagraph(paragraph) => validate_rich_text(&paragraph.text),
        InputRichBlock::InputRichBlockFooter(footer) => validate_rich_text(&footer.text),
        InputRichBlock::InputRichBlockPreformatted(pre) => validate_rich_text(&pre.text),
        InputRichBlock::InputRichBlockExpandableBlockQuotation(quote) => {
            validate_rich_text(&quote.text)?;
            if let Some(credit) = &quote.credit {
                validate_rich_text(credit)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockPullQuotation(quote) => {
            validate_rich_text(&quote.text)?;
            if let Some(credit) = &quote.credit {
                validate_rich_text(credit)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockBlockQuotation(quote) => {
            validate_blocks(&quote.blocks)?;
            if let Some(credit) = &quote.credit {
                validate_rich_text(credit)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockDetails(details) => {
            validate_rich_text(&details.summary)?;
            validate_blocks(&details.blocks)
        }
        InputRichBlock::InputRichBlockList(list) => {
            if list.items.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "list_items"));
            }
            for item in &list.items {
                validate_blocks(&item.blocks)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockTable(table) => {
            if table.cells.is_empty() || table.cells.iter().any(Vec::is_empty) {
                return Err(Error::local(ErrorKind::Validation, "table_cells"));
            }
            for row in &table.cells {
                for cell in row {
                    if cell.colspan.is_some_and(|span| span <= 0) {
                        return Err(Error::local(ErrorKind::Validation, "colspan"));
                    }
                    if cell.rowspan.is_some_and(|span| span <= 0) {
                        return Err(Error::local(ErrorKind::Validation, "rowspan"));
                    }
                    if let Some(text) = &cell.text {
                        validate_rich_text(text)?;
                    }
                }
            }
            if let Some(caption) = &table.caption {
                validate_rich_text(caption)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockButtons(buttons) => {
            if !(1..=8).contains(&buttons.buttons.len()) {
                return Err(Error::local(ErrorKind::Validation, "rich_buttons"));
            }
            for button in &buttons.buttons {
                validate_rich_button(button)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockPhoto(photo) => {
            if photo.photo.media.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "photo_media"));
            }
            if let Some(name) = photo.photo.media.strip_prefix("attach://")
                && !is_valid_attachment_name(name)
            {
                return Err(Error::local(ErrorKind::Validation, "attachment_name"));
            }
            if let Some(caption) = &photo.caption {
                validate_caption(caption)?;
            }
            Ok(())
        }
        InputRichBlock::InputRichBlockMathematicalExpression(math) => {
            if math.expression.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "math_expression"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_caption(caption: &RichBlockCaption) -> Result<()> {
    validate_rich_text(&caption.text)?;
    if let Some(credit) = &caption.credit {
        validate_rich_text(credit)?;
    }
    Ok(())
}

fn validate_rich_button(button: &RichMessageButton) -> Result<()> {
    if !validate_button_text(&button.text)? {
        return Err(Error::local(ErrorKind::Validation, "button_text"));
    }
    if let Some(data) = &button.callback_data
        && !(1..=64).contains(&data.len())
    {
        return Err(Error::local(ErrorKind::Validation, "callback_data"));
    }
    if let Some(url) = &button.url
        && url.is_empty()
    {
        return Err(Error::local(ErrorKind::Validation, "url"));
    }
    if let Some(copy) = &button.copy_text {
        let len = copy.text.chars().count();
        if !(1..=256).contains(&len) {
            return Err(Error::local(ErrorKind::Validation, "copy_text"));
        }
    }
    Ok(())
}

fn validate_button_text(text: &WireRichText) -> Result<bool> {
    match text {
        WireRichText::String(value) => Ok(!value.is_empty()),
        WireRichText::RichTextCustomEmoji(emoji) => {
            if emoji.custom_emoji_id.is_empty() || emoji.alternative_text.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "custom_emoji_id"));
            }
            Ok(true)
        }
        WireRichText::RichTextDateTime(dt) => {
            if !is_valid_date_time_format(&dt.date_time_format) {
                return Err(Error::local(ErrorKind::Validation, "date_time_format"));
            }
            validate_button_text(&dt.text)
        }
        WireRichText::Array(items) => {
            let mut has_content = false;
            for item in items {
                if validate_button_text(item)? {
                    has_content = true;
                }
            }
            Ok(has_content)
        }
        _ => Err(Error::local(ErrorKind::Validation, "button_text")),
    }
}

fn validate_rich_text(text: &WireRichText) -> Result<()> {
    match text {
        WireRichText::String(_) => Ok(()),
        WireRichText::Array(items) => {
            for item in items {
                validate_rich_text(item)?;
            }
            Ok(())
        }
        WireRichText::RichTextBold(node) => validate_rich_text(&node.text),
        WireRichText::RichTextItalic(node) => validate_rich_text(&node.text),
        WireRichText::RichTextUnderline(node) => validate_rich_text(&node.text),
        WireRichText::RichTextStrikethrough(node) => validate_rich_text(&node.text),
        WireRichText::RichTextSpoiler(node) => validate_rich_text(&node.text),
        WireRichText::RichTextMarked(node) => validate_rich_text(&node.text),
        WireRichText::RichTextCode(node) => validate_rich_text(&node.text),
        WireRichText::RichTextSubscript(node) => validate_rich_text(&node.text),
        WireRichText::RichTextSuperscript(node) => validate_rich_text(&node.text),
        WireRichText::RichTextUrl(node) => {
            if node.url.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "url"));
            }
            validate_rich_text(&node.text)
        }
        WireRichText::RichTextCustomEmoji(node) => {
            if node.custom_emoji_id.is_empty() || node.alternative_text.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "custom_emoji_id"));
            }
            Ok(())
        }
        WireRichText::RichTextDateTime(node) => {
            if !is_valid_date_time_format(&node.date_time_format) {
                return Err(Error::local(ErrorKind::Validation, "date_time_format"));
            }
            validate_rich_text(&node.text)
        }
        WireRichText::RichTextMathematicalExpression(node) => {
            if node.expression.is_empty() {
                return Err(Error::local(ErrorKind::Validation, "math_expression"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
