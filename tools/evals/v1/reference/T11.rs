pub fn solve(text: &str, range: std::ops::Range<usize>) -> tg::Result<tg::RichText> {
    tg::RichText::new(text).entity(range, tg::EntityKind::Bold)
}
