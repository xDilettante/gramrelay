use tg::{
    EffectOutcome,
    api::types::File,
    test_kit::{MockResponse, MockTransport, Step},
};

#[tokio::test]
async fn streams_only_bounded_bytes_and_does_not_repeat_partial_output() {
    let file = File::new("fixture".into(), "unique".into()).file_path("docs/shared.bin".into());
    let mock = MockTransport::new([Step::reply(
        "download",
        MockResponse::chunks([vec![0, 4], vec![255, 9]]),
    )])
    .unwrap();
    let mut output = Vec::new();
    let count = solution::solve(&mock.bot().unwrap(), &file, 4, &mut output)
        .await
        .unwrap();
    assert_eq!((count, output), (4, vec![0, 4, 255, 9]));
    mock.assert_finished().unwrap();
    let requests = mock.requests().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method(), "download");
    assert_eq!(requests[0].http_method(), "GET");

    let mock = MockTransport::new([Step::reply(
        "download",
        MockResponse::chunks([vec![1, 2], vec![3, 4]]),
    )])
    .unwrap();
    let mut output = Vec::new();
    assert!(
        solution::solve(&mock.bot().unwrap(), &file, 3, &mut output)
            .await
            .is_err()
    );
    assert!(output.len() <= 3);

    let mock = MockTransport::new([
        Step::reply(
            "download",
            MockResponse::chunks([vec![7, 8], vec![9, 10]]).fail_after_chunks(1),
        ),
        Step::reply("download", MockResponse::bytes(vec![1, 2])),
    ])
    .unwrap();
    let mut output = Vec::new();
    assert_eq!(
        solution::solve(&mock.bot().unwrap(), &file, 10, &mut output)
            .await
            .unwrap_err()
            .outcome(),
        EffectOutcome::Unknown
    );
    assert_eq!(output, [7, 8]);
    assert_eq!(mock.requests().unwrap().len(), 1);
}
