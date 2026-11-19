pub async fn solve(
    path: &std::path::Path,
    profile: &str,
    principal: &str,
    key: &str,
    payload: &serde_json::Value,
    receipt: serde_json::Value,
) -> tg_agent::journal::Result<Option<tg_agent::journal::Operation>> {
    use tg_agent::journal::{Completion, Journal, JournalOptions, OperationRequest, Preparation};
    let journal = Journal::open(path, JournalOptions::default()).await?;
    let request = OperationRequest::new(profile, principal, key, "send_message", payload)?;
    if let Preparation::Ready(prepared) = journal.prepare(request).await? {
        let flight = journal.begin(prepared).await?;
        journal
            .complete(flight, Completion::succeeded(receipt))
            .await?;
    }
    journal.close().await?;
    let journal = Journal::open(path, JournalOptions::default()).await?;
    let result = journal.get(profile, principal, key).await;
    journal.close().await?;
    result
}
