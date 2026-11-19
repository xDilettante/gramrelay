pub fn solve(input: &str) -> serde_json::Result<Option<String>> {
    let update: tg::Update = serde_json::from_str(input)?;
    Ok(update
        .callback_query
        .as_ref()
        .and_then(|q| q.message.as_ref())
        .and_then(|message| message.accessible())
        .and_then(|message| message.text.clone()))
}
