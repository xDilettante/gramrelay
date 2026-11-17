pub fn solve(
    input: &str,
    key: &str,
) -> serde_json::Result<(Option<serde_json::Value>, serde_json::Value)> {
    let message: tg::Message = serde_json::from_str(input)?;
    let field = message.unknown_fields().get(key).cloned();
    Ok((field, serde_json::to_value(message)?))
}
