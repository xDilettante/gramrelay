use crate::fixture::*;
use serde_json::json;
use std::time::Duration;
use tg::test_kit::{MockResponse, MockTransport, Step};

#[tokio::test]
async fn retries_transient_reads_only_within_two_attempts() {
    let successful = reply(
        "getMe",
        json!({"id":614,"is_bot":true,"first_name":"Fixture"}),
    );
    let mock = MockTransport::new([Step::transport_error("getMe"), successful]).unwrap();
    let user = solution::solve(&mock.bot().unwrap()).await.unwrap();
    assert_eq!(i64::from(user.id), 614);
    mock.assert_finished().unwrap();
    assert_eq!(mock.requests().unwrap().len(), 2);

    let mock = MockTransport::new((0..3).map(|_| Step::transport_error("getMe"))).unwrap();
    assert!(solution::solve(&mock.bot().unwrap()).await.is_err());
    assert_eq!(mock.requests().unwrap().len(), 2);

    let rejected = Step::reply(
        "getMe",
        MockResponse::json(json!({"ok":false,"error_code":401,"description":"fixture"}))
            .status(401),
    );
    let mock = MockTransport::new([rejected, Step::transport_error("getMe")]).unwrap();
    let error = solution::solve(&mock.bot().unwrap()).await.unwrap_err();
    assert_eq!(error.outcome(), tg::EffectOutcome::Rejected);
    assert_eq!(mock.requests().unwrap().len(), 1);

    let mock = MockTransport::new([Step::timeout("getMe")]).unwrap();
    let bot = mock.bot().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), solution::solve(&bot))
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(mock.requests().unwrap().len(), 1);
}
