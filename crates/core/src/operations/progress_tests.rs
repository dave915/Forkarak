#[test]
fn output_is_observable_before_exit_and_cancellation_still_reaps() {
    let fixture = Fixture::new();
    let progress = OperationProgress::default();
    let token = OperationCancellation::default();
    let op = fixture.op().with_progress(progress.clone()).with_cancellation(token.clone());
    let root = op.root.clone();
    let worker = thread::spawn(move || {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'live-out'; printf 'live-error' >&2; sleep 30"]);
        op.run_command(command)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = vec![];
    while Instant::now() < deadline {
        events.extend(progress.drain());
        if events.iter().any(|e| e.stream == OutputStream::Stderr) { break; }
        thread::sleep(Duration::from_millis(10));
    }
    token.cancel();
    assert!(worker.join().unwrap().unwrap_err().to_string().contains("Git in"));
    assert!(events.iter().all(|e| e.root == root));
    assert!(events.iter().any(|e| e.stream == OutputStream::Stdout && e.bytes == b"live-out"));
    assert!(events.iter().any(|e| e.stream == OutputStream::Stderr && e.bytes == b"live-error"));
}
#[test]
fn live_output_queue_is_bounded() {
    let progress = OperationProgress::default();
    for _ in 0..100 { progress.emit(Path::new("/fixture"), OutputStream::Stdout, &[b'x'; 8192]); }
    assert!(progress.dropped_bytes() > 0);
    assert!(progress.drain().iter().map(|e| e.bytes.len()).sum::<usize>() <= OUTPUT_LIMIT);
    assert!(progress.drain().is_empty());
}
