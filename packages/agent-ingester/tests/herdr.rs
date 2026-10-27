use agent_ingester::herdr::{Client, Error};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};
use tokio_util::sync::CancellationToken;

async fn server(
    handler: impl Fn(&Value) -> Value + Send + Sync + 'static,
) -> (
    tempfile::TempDir,
    Client,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            seen.lock().unwrap().push(request.clone());
            let response = handler(&request);
            reader
                .get_mut()
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    (dir, Client::new(path), calls, task)
}

#[tokio::test]
async fn exact_pid_beats_earlier_cwd_fallback_and_focus_uses_native_rpc() {
    let (_dir,client,calls,task)=server(|request| {
        let result=match request["method"].as_str().unwrap() {
            "ping"=>json!({"ok":true}),
            "pane.list"=>json!({"panes":[{"pane_id":"fallback","workspace_id":"w1","tab_id":"t1"},{"pane_id":"exact","workspace_id":"w2","tab_id":"t2","title":"Terminal"}]}),
            "pane.process_info" if request["params"]["pane_id"]=="fallback"=>json!({"foreground_processes":[{"pid":1,"name":"Pi","cwd":"/work/a/../repo"}]}),
            "pane.process_info"=>json!({"process_info":{"foreground_processes":[{"pid":42,"name":"other","cwd":"/elsewhere"}]}}),
            "pane.focus"=>json!({"ok":true}),
            method=>panic!("unexpected {method}"),
        };json!({"id":request["id"],"result":result})
    }).await;
    let token = CancellationToken::new();
    client.ping(&token).await.unwrap();
    let pane = client
        .find_pane(&token, 42, "/work/repo", "pi")
        .await
        .unwrap();
    assert_eq!(
        (
            pane.pane_id.as_str(),
            pane.workspace_id.as_str(),
            pane.tab_id.as_str(),
            pane.title.as_str()
        ),
        ("exact", "w2", "t2", "Terminal")
    );
    client.focus_pane(&token, &pane.pane_id).await.unwrap();
    let pane = client
        .find_pane(&token, 0, "/work/repo", "PI")
        .await
        .unwrap();
    assert_eq!(pane.pane_id, "fallback");
    assert!(matches!(
        client.find_pane(&token, 0, "", "pi").await,
        Err(Error::PaneNotFound)
    ));
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0]["params"], json!({}));
    assert_eq!(calls[4]["params"], json!({"pane_id":"exact"}));
    assert_ne!(calls[0]["id"], calls[1]["id"]);
    task.abort();
}

#[tokio::test]
async fn native_result_variants_failures_and_response_identity() {
    let (_dir, client, calls, task) = server(|request| {
        let result = match request["method"].as_str().unwrap() {
            "pane.list" => json!([{"pane_id":"broken"},{"pane_id":"fallback"}]),
            "pane.process_info" if request["params"]["pane_id"] == "broken" => {
                return json!({"id":request["id"],"error":{"code":7,"message":"unavailable pane"}});
            }
            "pane.process_info" => {
                json!({"foreground_processes":[{"pid":3,"argv0":"/bin/codex","cwd":"/project"}]})
            }
            _ => return json!({"id":"wrong-response","result":{}}),
        };
        json!({"id":request["id"],"result":result})
    })
    .await;
    let token = CancellationToken::new();
    assert_eq!(
        client
            .find_pane(&token, 0, "/project/.", "codex")
            .await
            .unwrap()
            .pane_id,
        "fallback"
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["method"] == "pane.list")
            .count(),
        1
    );
    assert!(matches!(
        client.ping(&token).await,
        Err(Error::ResponseIdentity { .. })
    ));
    task.abort();
    let (_dir, client, _, task) =
        server(|request| json!({"id":request["id"],"error":{"code":403,"message":"denied"}})).await;
    assert!(matches!(
        client.ping(&token).await,
        Err(Error::Rpc { code: 403, .. })
    ));
    task.abort();
    let (_dir, client, _, task) =
        server(|request| json!({"id":request["id"],"result":{"panes":[]}})).await;
    assert!(matches!(
        client.find_pane(&token, 1, "/", "pi").await,
        Err(Error::PaneNotFound)
    ));
    task.abort();
    let (_dir, client, _, task) = server(|request| json!({"id":request["id"],"result":42})).await;
    assert!(matches!(
        client.find_pane(&token, 1, "/", "pi").await,
        Err(Error::Json(_))
    ));
    task.abort();
}

#[tokio::test]
async fn local_rpc_deadline_cancellation_and_unavailable_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing");
    let token = CancellationToken::new();
    assert!(matches!(
        Client::new(&path).ping(&token).await,
        Err(Error::Unavailable(_))
    ));
    let listener = UnixListener::bind(&path).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(stream);
    });
    let client = Client::new(&path).with_timeout(Duration::from_millis(20));
    assert!(matches!(client.ping(&token).await, Err(Error::Timeout)));
    task.abort();
    let listener = UnixListener::bind(dir.path().join("pending")).unwrap();
    let (accepted, connected) = tokio::sync::oneshot::channel();
    let pending_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        accepted.send(()).unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(stream);
    });
    let task = tokio::spawn({
        let token = token.clone();
        async move {
            connected.await.unwrap();
            token.cancel();
        }
    });
    assert!(matches!(
        Client::new(dir.path().join("pending")).ping(&token).await,
        Err(Error::Cancelled)
    ));
    task.await.unwrap();
    pending_server.abort();
    assert!(matches!(client.ping(&token).await, Err(Error::Cancelled)));
}

#[tokio::test]
async fn incomplete_and_invalid_responses_remain_typed_transport_errors() {
    for (name, bytes) in [
        ("truncated", b"{\"id\":\"req-1\"}".as_slice()),
        ("invalid", b"not-json\n".as_slice()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let listener = UnixListener::bind(&path).unwrap();
        let bytes = bytes.to_vec();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            reader.get_mut().write_all(&bytes).await.unwrap();
        });
        let error = Client::new(path)
            .ping(&CancellationToken::new())
            .await
            .unwrap_err();
        match name {
            "truncated" => assert!(
                matches!(error, Error::Io(error) if error.kind()==std::io::ErrorKind::UnexpectedEof)
            ),
            "invalid" => assert!(matches!(error, Error::Json(_))),
            _ => unreachable!(),
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn activation_passes_application_names_as_one_literal_subprocess_argument() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("capture");
    let output = dir.path().join("argv");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$CAPTURE\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let app = "Terminal\"\n do shell script \"false\" --";
    let mut command =
        agent_ingester::herdr::activation_command(&executable, std::ffi::OsStr::new(app));
    assert!(
        command
            .env("CAPTURE", &output)
            .status()
            .await
            .unwrap()
            .success()
    );
    let bytes = std::fs::read(output).unwrap();
    let args: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    assert_eq!(args.len(), 5);
    assert_eq!(args[0], b"-e");
    assert!(!std::str::from_utf8(args[1]).unwrap().contains(app));
    assert_eq!(args[2], b"--");
    assert_eq!(args[3], app.as_bytes());
}
