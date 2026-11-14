//! FR-120/FR-125 — the real `pluryprint-bridge` executable, driven the way a
//! browser drives it: the caller's origin as the first argument, framed JSON
//! on stdin, framed JSON back on stdout, stdin closed at the end.
//!
//! Hermetic: the host's folders are a temporary directory
//! (`PLURYPRINT_BRIDGE_DATA_DIR`) and the app is never started
//! (`PLURYPRINT_BRIDGE_NO_LAUNCH=1`, which writes the handoff and stops
//! there). Both variables exist for tests only.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine as _;
use serde_json::{json, Value};

use pluryprint_lib::bridge::{validate::sha256_hex, PLURYFACT_ORIGIN};
use pluryprint_lib::cli::handoff;

fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap();
    let mut out = (body.len() as u32).to_ne_bytes().to_vec();
    out.extend(body);
    out
}

/// Split the bridge's stdout into its frames; anything that is not a whole
/// frame of JSON fails the test (stdout is the protocol channel only).
fn frames(mut out: &[u8]) -> Vec<Value> {
    let mut msgs = Vec::new();
    while !out.is_empty() {
        assert!(out.len() >= 4, "a partial length prefix on stdout");
        let len = u32::from_ne_bytes(out[..4].try_into().unwrap()) as usize;
        assert!(len <= 1024 * 1024, "a reply over 1 MB");
        assert!(out.len() >= 4 + len, "a partial frame on stdout");
        msgs.push(serde_json::from_slice(&out[4..4 + len]).expect("each frame is JSON"));
        out = &out[4 + len..];
    }
    msgs
}

/// Run the bridge with `origin`, feed it `input`, close stdin, and return its
/// exit code and replies.
fn run(data: &Path, origin: Option<&str>, input: &[u8]) -> (i32, Vec<Value>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pluryprint-bridge"));
    if let Some(origin) = origin {
        cmd.arg(origin);
    }
    let mut child = cmd
        .env("PLURYPRINT_BRIDGE_DATA_DIR", data)
        .env("PLURYPRINT_BRIDGE_NO_LAUNCH", "1")
        .env("PLURYPRINT_SKIP_MIGRATION", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pluryprint-bridge");
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&input).unwrap();
        // Dropping stdin is the browser closing the port.
    });
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    writer.join().unwrap();
    let status = child.wait().unwrap();
    (status.code().unwrap_or(-1), frames(&stdout))
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// A PDF big enough to need several chunks at a small chunk size.
fn pdf(n: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n".to_vec();
    v.extend(std::iter::repeat_n(b'x', n));
    v.extend_from_slice(b"\n%%EOF\n");
    v
}

/// Two connections. The first says hello and opens a job, then closes the
/// port: the job is discarded. The second streams like the extension does
/// (one message, its reply, the next): a chunked PDF and an XML document,
/// committed, become a handoff the app's own reader accepts.
#[test]
fn fr_120_fr_125_hello_begin_put_commit_produces_a_handoff_the_app_accepts() {
    let tmp = tempfile::tempdir().unwrap();
    let doc0 = pdf(300_000);
    let doc1 = b"<?xml version=\"1.0\"?><factura><ruc>0999999999001</ruc></factura>".to_vec();

    let mut input = Vec::new();
    input.extend(frame(&json!({"v":1,"id":"h","type":"hello","client":"pluryfact","clientVersion":"0.2.0","protocols":[1]})));
    input.extend(frame(&json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"Recibidos · octubre 2026",
        "action":"queue","profileId":"builtin-duplex-bw","printerId":null,"overrides":{"copies":2,"color":"mono"},"expectedFiles":2})));
    let (code, replies) = run(tmp.path(), Some(PLURYFACT_ORIGIN), &input);
    assert_eq!(code, 0);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["ok"], true, "{}", replies[0]);
    assert_eq!(replies[0]["protocol"], 1);
    let first_job = replies[1]["jobId"].as_str().unwrap().to_string();
    // The connection closed before commitJob: the job was discarded.
    assert!(!tmp.path().join("inbox").join(&first_job).exists());

    // The streaming client: write a message, read its reply, decide the next.
    let mut child = Command::new(env!("CARGO_BIN_EXE_pluryprint-bridge"))
        .arg(PLURYFACT_ORIGIN)
        .env("PLURYPRINT_BRIDGE_DATA_DIR", tmp.path())
        .env("PLURYPRINT_BRIDGE_NO_LAUNCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut ask = |msg: Value| -> Value {
        stdin.write_all(&frame(&msg)).unwrap();
        stdin.flush().unwrap();
        let mut len = [0u8; 4];
        stdout.read_exact(&mut len).unwrap();
        let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
        stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    };
    let r = ask(
        json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"Recibidos · octubre 2026",
        "action":"queue","profileId":"builtin-duplex-bw","printerId":null,"overrides":{"copies":2,"color":"mono"},"expectedFiles":2}),
    );
    let job = r["jobId"].as_str().unwrap().to_string();
    let chunk = 100_000;
    let chunks = doc0.len().div_ceil(chunk);
    for c in 0..chunks {
        let piece = &doc0[c * chunk..((c + 1) * chunk).min(doc0.len())];
        let r = ask(
            json!({"v":1,"id":format!("p{c}"),"type":"putFile","jobId":job,"index":0,
            "name":"RIDE 001-001-000000123.pdf","kind":"pdf","size":doc0.len(),"sha256":sha256_hex(&doc0),
            "chunk":c,"chunks":chunks,"data":b64(piece)}),
        );
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["complete"], c + 1 == chunks);
        assert_eq!(r["received"], ((c + 1) * chunk).min(doc0.len()));
    }
    let r = ask(
        json!({"v":1,"id":"x","type":"putFile","jobId":job,"index":1,"name":"factura.xml","kind":"xml",
        "size":doc1.len(),"sha256":sha256_hex(&doc1),"chunk":0,"chunks":1,"data":b64(&doc1)}),
    );
    assert_eq!(r["complete"], true, "{r}");
    let r = ask(json!({"v":1,"id":"c","type":"commitJob","jobId":job}));
    assert_eq!(
        r,
        json!({"v":1,"id":"c","type":"commitJob","ok":true,"state":"handed-off","files":2})
    );
    drop(stdin);
    let mut rest = Vec::new();
    stdout.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "nothing after the last reply: {rest:?}");
    assert_eq!(child.wait().unwrap().code(), Some(0));

    // The handoff is one the app's own reader accepts.
    let handoff_dir = tmp.path().join("handoff");
    let file = handoff_dir.join(format!("pluryfact-{job}.json"));
    assert!(handoff::is_within_dir(&handoff_dir, &file));
    let h = handoff::read_file(&file).expect("valid handoff");
    assert!(handoff::is_fresh(
        &h.invoked_at,
        time::OffsetDateTime::now_utc(),
        handoff::MAX_HANDOFF_AGE
    ));
    assert_eq!(h.schema_version, 1);
    assert_eq!(h.source, "pluryfact");
    assert_eq!(h.label.as_deref(), Some("Recibidos · octubre 2026"));
    assert_eq!(h.overrides.as_ref().and_then(|o| o.copies), Some(2));
    assert_eq!(h.paths.len(), 2);
    assert_eq!(std::fs::read(&h.paths[0]).unwrap(), doc0);
    assert_eq!(std::fs::read(&h.paths[1]).unwrap(), doc1);
    for p in &h.paths {
        assert!(
            Path::new(p).starts_with(tmp.path().join("inbox").join(&job)),
            "{p}"
        );
    }
    assert_ne!(job, first_job, "every job gets a fresh id");
    // Only the committed job is in the inbox.
    assert_eq!(
        std::fs::read_dir(tmp.path().join("inbox")).unwrap().count(),
        1
    );
}

#[test]
fn fr_120_an_unknown_origin_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let hello = frame(
        &json!({"v":1,"id":"1","type":"hello","client":"x","clientVersion":"1","protocols":[1]}),
    );
    for origin in [
        Some("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"),
        Some("https://example.com/"),
        None,
    ] {
        let (code, replies) = run(tmp.path(), origin, &hello);
        assert_eq!(code, 0);
        assert_eq!(
            replies[0]["error"]["code"], "origin_not_allowed",
            "{origin:?}"
        );
    }
}

/// An origin a manifest of this host allows (here the test manifest in the
/// data folder, as `bridge install --extra-origin` would add one) is
/// accepted.
#[test]
fn fr_127_an_origin_the_manifest_allows_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let dev = format!("chrome-extension://{}/", "k".repeat(32));
    let manifest = pluryprint_lib::bridge::register::Manifest::new(
        "/x/pluryprint-bridge".into(),
        vec![PLURYFACT_ORIGIN.into(), dev.clone()],
    );
    std::fs::write(
        tmp.path()
            .join(pluryprint_lib::bridge::register::manifest_file_name()),
        manifest.to_bytes(),
    )
    .unwrap();
    let hello = frame(
        &json!({"v":1,"id":"1","type":"hello","client":"pluryfact","clientVersion":"0.2.0","protocols":[1]}),
    );
    let (_, replies) = run(tmp.path(), Some(&dev), &hello);
    assert_eq!(replies[0]["ok"], true, "{}", replies[0]);
}

/// Windows passes `--parent-window=<hwnd>` after the origin; it is ignored.
#[test]
fn fr_120_the_parent_window_argument_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pluryprint-bridge"))
        .args([PLURYFACT_ORIGIN, "--parent-window=0"])
        .env("PLURYPRINT_BRIDGE_DATA_DIR", tmp.path())
        .env("PLURYPRINT_BRIDGE_NO_LAUNCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&frame(&json!({"v":1,"id":"1","type":"listProfiles"})))
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let replies = frames(&out.stdout);
    assert_eq!(replies[0]["type"], "listProfiles");
    assert_eq!(replies[0]["ok"], true, "{}", replies[0]);
}

/// Garbage on the pipe is answered, not fatal; a truncated frame ends the
/// process with 1 and discards the open job.
#[test]
fn fr_120_broken_input_is_answered_or_ends_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let mut input = Vec::new();
    let junk = b"definitely not json";
    input.extend((junk.len() as u32).to_ne_bytes());
    input.extend(junk);
    input.extend(frame(
        &json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"x",
        "action":"configure","profileId":null,"printerId":null,"expectedFiles":1}),
    ));
    input.extend(100u32.to_ne_bytes());
    input.extend(b"{\"v\":1");
    let (code, replies) = run(tmp.path(), Some(PLURYFACT_ORIGIN), &input);
    assert_eq!(code, 1);
    assert_eq!(replies[0]["error"]["code"], "bad_request");
    let job = replies[1]["jobId"].as_str().unwrap();
    assert!(!tmp.path().join("inbox").join(job).exists());
}
