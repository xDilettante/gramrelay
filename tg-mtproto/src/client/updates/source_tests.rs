use super::*;
#[tokio::test]
async fn closed_loss_signal_is_backend_failure_and_only_sticky_true_is_loss() {
    let (_sender, updates) = mpsc::channel(1);
    let (sender, loss) = watch::channel(false);
    let mut source = Source { updates, loss };
    drop(sender);
    assert!(
        matches!(source.receive(tokio::time::Instant::now()).await,Err(error) if error.kind()==ClientErrorKind::Backend)
    );
    let (_sender, updates) = mpsc::channel(1);
    let (sender, loss) = watch::channel(true);
    let mut source = Source { updates, loss };
    assert!(matches!(
        source.receive(tokio::time::Instant::now()).await,
        Ok(Received::Loss)
    ));
    drop(sender);
    let (sender, updates) = mpsc::channel(1);
    let (_signal, loss) = watch::channel(false);
    let mut source = Source { updates, loss };
    drop(sender);
    assert!(
        matches!(source.receive(tokio::time::Instant::now()).await,Err(error) if error.kind()==ClientErrorKind::Backend)
    );
}
