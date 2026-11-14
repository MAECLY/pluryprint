//! The bridge's state machine against a fake host and a temporary folder:
//! the whole exchange, and every error code of the protocol.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use base64::Engine as _;
use serde_json::{json, Value};

use super::*;
use crate::bridge::validate::sha256_hex;

#[derive(Clone)]
struct FakeHost {
    data: PathBuf,
    launches: Rc<RefCell<Vec<Vec<String>>>>,
    fail_launch: bool,
    fail_printers: bool,
    printers: Vec<PrinterEntry>,
}

impl FakeHost {
    fn new(data: &Path) -> Self {
        Self {
            data: data.to_path_buf(),
            launches: Rc::default(),
            fail_launch: false,
            fail_printers: false,
            printers: vec![PrinterEntry {
                id: "HP_LaserJet".into(),
                name: "HP LaserJet".into(),
                is_default: true,
                capabilities: Some(protocol::PrinterCapabilities {
                    duplex: true,
                    color: false,
                    papers: vec!["A4".into(), "Letter".into()],
                }),
            }],
        }
    }
}

impl Host for FakeHost {
    fn printers(&self) -> Result<Vec<PrinterEntry>, String> {
        if self.fail_printers {
            Err("spooler unavailable".into())
        } else {
            Ok(self.printers.clone())
        }
    }
    fn profiles(&self) -> Result<Vec<ProfileEntry>, String> {
        Ok(vec![
            ProfileEntry {
                id: "builtin-quick".into(),
                name: "Quick".into(),
                is_default: true,
            },
            ProfileEntry {
                id: "builtin-duplex-bw".into(),
                name: "Duplex BW".into(),
                is_default: false,
            },
        ])
    }
    fn data_dir(&self) -> PathBuf {
        self.data.clone()
    }
    fn launch(&self, args: &[String]) -> Result<(), String> {
        if self.fail_launch {
            return Err("cannot start the app".into());
        }
        self.launches.borrow_mut().push(args.to_vec());
        Ok(())
    }
}

fn send<H: Host>(s: &mut Session<H>, msg: Value) -> Value {
    let reply = s.handle_bytes(msg.to_string().as_bytes());
    assert!(reply.len() <= crate::bridge::framing::MAX_OUTBOUND_BYTES);
    serde_json::from_slice(&reply).expect("every reply is JSON")
}

fn code(reply: &Value) -> &str {
    assert_eq!(reply["ok"], false, "{reply}");
    reply["error"]["code"].as_str().unwrap()
}

fn begin<H: Host>(s: &mut Session<H>, expected: u32) -> String {
    begin_with(s, json!({"expectedFiles": expected}))
}

fn begin_with<H: Host>(s: &mut Session<H>, extra: Value) -> String {
    let mut msg = json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact",
        "label":"Recibidos · octubre 2026","action":"configure","expectedFiles":1});
    for (k, v) in extra.as_object().unwrap() {
        msg[k] = v.clone();
    }
    let r = send(s, msg);
    assert_eq!(r["ok"], true, "{r}");
    r["jobId"].as_str().unwrap().to_string()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Send `doc` as document `index` in `chunks` pieces; return the last reply.
fn put_doc<H: Host>(
    s: &mut Session<H>,
    job: &str,
    index: u32,
    name: &str,
    kind: &str,
    doc: &[u8],
    chunks: u32,
) -> Value {
    let size = doc.len();
    let per = size.div_ceil(chunks as usize).max(1);
    let mut last = Value::Null;
    for c in 0..chunks as usize {
        let piece = &doc[(c * per).min(size)..((c + 1) * per).min(size)];
        last = send(
            s,
            json!({"v":1,"id":format!("p{index}-{c}"),"type":"putFile","jobId":job,"index":index,
                   "name":name,"kind":kind,"size":size,"sha256":sha256_hex(doc),
                   "chunk":c,"chunks":chunks,"data":b64(piece)}),
        );
        if last["ok"] != true {
            return last;
        }
    }
    last
}

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";
const XML: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><autorizacion><estado>AUTORIZADO</estado><comprobante><![CDATA[<factura id=\"comprobante\" version=\"1.1.0\"><infoTributaria><ruc>0999999999001</ruc></infoTributaria></factura>]]></comprobante></autorizacion>";
const TXT: &[u8] = b"Reporte Pluryfact\r\nRecibidos: 2\r\n";

#[test]
fn fr_120_hello_advertises_the_protocol_and_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let r = send(
        &mut s,
        json!({"v":1,"id":"1","type":"hello","client":"pluryfact","clientVersion":"0.2.0","protocols":[1]}),
    );
    assert_eq!(r["ok"], true);
    assert_eq!(r["host"], "pluryprint");
    assert_eq!(r["hostVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["protocol"], 1);
    assert_eq!(
        r["capabilities"],
        json!([
            "listPrinters",
            "listProfiles",
            "beginJob",
            "putFile",
            "commitJob",
            "abortJob"
        ])
    );
    assert_eq!(
        r["limits"],
        json!({"maxFileBytes":52428800,"maxFiles":5000,"maxChunkBytes":16777216,"maxJobBytes":1073741824})
    );
    let r = send(
        &mut s,
        json!({"v":1,"id":"2","type":"hello","client":"pluryfact","clientVersion":"9","protocols":[2,3]}),
    );
    assert_eq!(code(&r), "unsupported_version");
}

#[test]
fn fr_120_lists_printers_and_profiles() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let r = send(&mut s, json!({"v":1,"id":"1","type":"listPrinters"}));
    assert_eq!(
        r["printers"],
        json!([{"id":"HP_LaserJet","name":"HP LaserJet","isDefault":true,
                "capabilities":{"duplex":true,"color":false,"papers":["A4","Letter"]}}])
    );
    let r = send(&mut s, json!({"v":1,"id":"2","type":"listProfiles"}));
    assert_eq!(
        r["profiles"][0],
        json!({"id":"builtin-quick","name":"Quick","isDefault":true})
    );
}

/// The whole happy path: three kinds, chunked, committed; the handoff is one
/// the app's own reader accepts, in print order, and the app is started
/// with the context-menu command line.
#[test]
fn fr_120_fr_125_a_committed_job_becomes_a_handoff_the_app_accepts() {
    let tmp = tempfile::tempdir().unwrap();
    let host = FakeHost::new(tmp.path());
    let launches = host.launches.clone();
    let mut s = Session::new(host, true);
    let job = begin_with(
        &mut s,
        json!({"expectedFiles":3,"action":"print-default","profileId":"builtin-duplex-bw",
               "printerId":"HP_LaserJet","overrides":{"copies":2,"duplex":"long-edge","color":"mono",
               "paper":"A4","orientation":"portrait","collate":false}}),
    );

    let r = put_doc(
        &mut s,
        &job,
        0,
        "../../RIDE 001-001-000000123.pdf",
        "pdf",
        PDF,
        3,
    );
    assert_eq!(r["complete"], true, "{r}");
    assert_eq!(r["received"], PDF.len());
    // Documents may arrive in any order; the print order is the index.
    assert_eq!(
        put_doc(&mut s, &job, 2, "reporte.txt", "txt", TXT, 1)["complete"],
        true
    );
    let r = send(
        &mut s,
        json!({"v":1,"id":"c0","type":"commitJob","jobId":job}),
    );
    assert_eq!(code(&r), "job_incomplete");
    assert_eq!(
        put_doc(&mut s, &job, 1, "factura.xml", "xml", XML, 2)["complete"],
        true
    );

    let r = send(
        &mut s,
        json!({"v":1,"id":"c1","type":"commitJob","jobId":job}),
    );
    assert_eq!(
        r,
        json!({"v":1,"id":"c1","type":"commitJob","ok":true,"state":"handed-off","files":3})
    );
    assert_eq!(s.open_jobs(), 0);

    let handoff_dir = tmp.path().join("handoff");
    let file = handoff_dir.join(format!("pluryfact-{job}.json"));
    let h = crate::cli::handoff::read_file(&file).expect("the app's reader accepts it");
    assert!(crate::cli::handoff::is_within_dir(&handoff_dir, &file));
    assert!(crate::cli::handoff::is_fresh(
        &h.invoked_at,
        time::OffsetDateTime::now_utc(),
        crate::cli::handoff::MAX_HANDOFF_AGE
    ));
    assert_eq!(h.source, "pluryfact");
    assert_eq!(
        h.action,
        Some(crate::cli::args::ContextAction::PrintDefault)
    );
    assert_eq!(h.profile_id.as_deref(), Some("builtin-duplex-bw"));
    assert_eq!(h.label.as_deref(), Some("Recibidos · octubre 2026"));
    let o = h.overrides.clone().unwrap();
    assert_eq!(o.printer.as_deref(), Some("HP_LaserJet"));
    assert_eq!(o.copies, Some(2));
    assert_eq!(o.color, Some(crate::queue::job::ColorMode::Monochrome));
    assert_eq!(o.paper, Some(crate::queue::job::PaperSize::A4));
    assert_eq!(o.collate, Some(false));

    let job_dir = tmp.path().join("inbox").join(&job);
    let want = [
        "0000-RIDE 001-001-000000123.pdf",
        "0001-factura.xml",
        "0002-reporte.txt",
    ];
    let got: Vec<String> = h
        .paths
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(got, want);
    for (p, bytes) in h.paths.iter().zip([PDF, XML, TXT]) {
        assert!(Path::new(p).starts_with(&job_dir), "{p}");
        assert_eq!(std::fs::read(p).unwrap(), bytes);
    }
    // Nothing else is left in the job folder (no .part files).
    assert_eq!(std::fs::read_dir(&job_dir).unwrap().count(), 3);

    // The selection the app builds from it carries the source, the label
    // and the overrides through to the UI.
    let sel =
        crate::cli::selection_from_payload(h, crate::cli::args::ContextAction::Configure, None);
    assert_eq!(sel.source.as_deref(), Some("pluryfact"));
    assert_eq!(sel.label.as_deref(), Some("Recibidos · octubre 2026"));
    assert_eq!(sel.overrides.unwrap().copies, Some(2));

    let launches = launches.borrow();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches[0],
        crate::bridge::launch::gui_args(
            &file,
            crate::cli::args::ContextAction::PrintDefault,
            Some("builtin-duplex-bw")
        )
    );

    // The connection closing does not touch a committed job.
    s.close();
    assert!(job_dir.is_dir());
}

#[test]
fn fr_120_an_aborted_or_abandoned_job_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let a = begin(&mut s, 2);
    let b = begin(&mut s, 2);
    put_doc(&mut s, &a, 0, "a.pdf", "pdf", PDF, 1);
    put_doc(&mut s, &b, 0, "b.pdf", "pdf", PDF, 1);
    let r = send(&mut s, json!({"v":1,"id":"x","type":"abortJob","jobId":a}));
    assert_eq!(r["state"], "aborted");
    assert!(!tmp.path().join("inbox").join(&a).exists());
    let r = send(&mut s, json!({"v":1,"id":"x","type":"abortJob","jobId":a}));
    assert_eq!(code(&r), "unknown_job");

    assert!(tmp.path().join("inbox").join(&b).exists());
    s.close();
    assert!(
        !tmp.path().join("inbox").join(&b).exists(),
        "EOF discards it"
    );
    assert!(
        !tmp.path().join("handoff").exists(),
        "nothing was handed off"
    );
}

#[test]
fn fr_120_origin_not_allowed_refuses_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), false);
    for msg in [
        json!({"v":1,"id":"1","type":"hello","client":"x","clientVersion":"1","protocols":[1]}),
        json!({"v":1,"id":"2","type":"listPrinters"}),
        json!({"v":1,"id":"3","type":"beginJob","source":"pluryfact","label":"","action":"configure","expectedFiles":1}),
    ] {
        let r = send(&mut s, msg);
        assert_eq!(code(&r), "origin_not_allowed");
    }
    assert!(!tmp.path().join("inbox").exists());
}

#[test]
fn fr_120_unknown_type_and_bad_envelopes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    assert_eq!(
        code(&send(&mut s, json!({"v":1,"id":"1","type":"jobStatus"}))),
        "unknown_type"
    );
    assert_eq!(
        code(&send(&mut s, json!({"v":2,"id":"1","type":"hello"}))),
        "unsupported_version"
    );
    let r: Value = serde_json::from_slice(&s.handle_bytes(b"\x00garbage")).unwrap();
    assert_eq!(
        r,
        json!({"v":1,"id":"","type":"","ok":false,
        "error":{"code":"bad_request","message":"the message is not valid JSON"}})
    );
    let r: Value = serde_json::from_slice(&s.oversized(70_000_000)).unwrap();
    assert_eq!(code(&r), "too_large");
}

#[test]
fn fr_120_begin_job_validates_its_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let base = json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"x",
                      "action":"configure","expectedFiles":1});
    let with = |k: &str, v: Value| {
        let mut m = base.clone();
        m[k] = v;
        m
    };
    let cases = [
        (with("source", json!("other")), "bad_request"),
        (with("label", json!("x".repeat(121))), "bad_request"),
        (with("label", json!("a\u{0007}b")), "bad_request"),
        (with("expectedFiles", json!(0)), "bad_request"),
        (with("expectedFiles", json!(5001)), "too_many_files"),
        (with("profileId", json!("a\nb")), "bad_request"),
        (with("printerId", json!("p".repeat(300))), "bad_request"),
        (with("overrides", json!({"copies":0})), "bad_request"),
        (with("overrides", json!({"copies":1000})), "bad_request"),
        (with("overrides", json!({"paper":""})), "bad_request"),
        (
            with("overrides", json!({"paper":"A\u{0001}4"})),
            "bad_request",
        ),
        (with("overrides", json!({"duplex":"both"})), "bad_request"),
        (with("overrides", json!({"scale":"fit"})), "bad_request"),
        (with("action", json!("print-now")), "bad_request"),
    ];
    for (msg, want) in cases {
        assert_eq!(code(&send(&mut s, msg.clone())), want, "{msg}");
    }
    assert!(
        !tmp.path().join("inbox").exists()
            || std::fs::read_dir(tmp.path().join("inbox")).unwrap().count() == 0
    );

    // A label of exactly 120 characters, blank ids and an empty override
    // set are fine.
    let ok = send(
        &mut s,
        json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"ñ".repeat(120),
               "action":"queue","profileId":"  ","printerId":null,"overrides":{},"expectedFiles":5000}),
    );
    assert_eq!(ok["ok"], true, "{ok}");

    // At most four open jobs per connection.
    for _ in 0..3 {
        begin(&mut s, 1);
    }
    assert_eq!(
        code(&send(&mut s, base.clone())),
        "bad_request",
        "a fifth open job is refused"
    );
}

#[test]
fn fr_121_put_file_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let job = begin(&mut s, 2);
    let sha = sha256_hex(PDF);
    let put = |chunk: u32, chunks: u32, extra: Value| {
        let mut m = json!({"v":1,"id":"p","type":"putFile","jobId":job,"index":0,"name":"a.pdf",
                           "kind":"pdf","size":PDF.len(),"sha256":sha,"chunk":chunk,"chunks":chunks,
                           "data":b64(&PDF[..10])});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    let cases = [
        (put(0, 1, json!({"jobId":"nope"})), "unknown_job"),
        (put(0, 1, json!({"index":2})), "too_many_files"),
        (put(0, 1, json!({"index":5000})), "too_many_files"),
        (put(0, 1, json!({"size":MAX_FILE_BYTES + 1})), "too_large"),
        (put(0, 0, json!({})), "bad_request"),
        (put(2, 2, json!({})), "bad_request"),
        (put(0, 1000, json!({})), "bad_request"),
        (put(0, 1, json!({"sha256":"ABC"})), "bad_request"),
        (
            put(0, 1, json!({"sha256":sha.to_uppercase()})),
            "bad_request",
        ),
        (put(0, 1, json!({"data":"not base64!"})), "bad_request"),
        (put(1, 2, json!({})), "bad_request"),
        (
            put(0, 1, json!({"data":"A".repeat(23_000_000)})),
            "too_large",
        ),
    ];
    for (msg, want) in cases {
        let r = send(&mut s, msg.clone());
        assert_eq!(
            code(&r),
            want,
            "{}",
            &msg.to_string()[..msg.to_string().len().min(300)]
        );
    }

    // Out of order and changing metadata mid-document.
    assert_eq!(send(&mut s, put(0, 3, json!({})))["ok"], true);
    assert_eq!(code(&send(&mut s, put(2, 3, json!({})))), "bad_request");
    assert_eq!(
        code(&send(&mut s, put(1, 3, json!({"kind":"xml"})))),
        "bad_request"
    );
    assert_eq!(
        code(&send(&mut s, put(1, 3, json!({"size":99})))),
        "bad_request"
    );
    // More bytes than declared.
    assert_eq!(
        code(&send(&mut s, put(1, 3, json!({"data":b64(&[b'x'; 64])})))),
        "bad_request"
    );
    // A restart from chunk 0 is allowed and replaces the partial document.
    assert_eq!(
        send(&mut s, put(0, 1, json!({"data":b64(PDF)})))["complete"],
        true
    );
    // …but a complete document is not received twice.
    assert_eq!(
        code(&send(&mut s, put(0, 1, json!({"data":b64(PDF)})))),
        "bad_request"
    );
}

#[test]
fn fr_121_content_and_checksum_failures_discard_the_document() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let job = begin(&mut s, 4);
    let job_dir = tmp.path().join("inbox").join(&job);

    let r = put_doc(
        &mut s,
        &job,
        0,
        "fake.pdf",
        "pdf",
        b"<html>not a pdf</html>",
        1,
    );
    assert_eq!(code(&r), "bad_pdf");
    let r = put_doc(
        &mut s,
        &job,
        1,
        "x.xml",
        "xml",
        b"<!DOCTYPE r [<!ENTITY a \"b\">]><r>&a;</r>",
        1,
    );
    assert_eq!(code(&r), "bad_request");
    let r = put_doc(&mut s, &job, 2, "r.txt", "txt", b"bin\x00ary", 1);
    assert_eq!(code(&r), "bad_request");
    // Wrong hash: right size, wrong content.
    let r = send(
        &mut s,
        json!({"v":1,"id":"p","type":"putFile","jobId":job,"index":3,"name":"a.pdf","kind":"pdf",
               "size":PDF.len(),"sha256":sha256_hex(b"other"),"chunk":0,"chunks":1,"data":b64(PDF)}),
    );
    assert_eq!(code(&r), "checksum_mismatch");
    assert_eq!(
        std::fs::read_dir(&job_dir).unwrap().count(),
        0,
        "nothing kept"
    );

    // Each can be sent again, correctly, afterwards.
    assert_eq!(
        put_doc(&mut s, &job, 3, "a.pdf", "pdf", PDF, 1)["complete"],
        true
    );
    assert_eq!(
        put_doc(&mut s, &job, 0, "a.pdf", "pdf", PDF, 1)["complete"],
        true
    );
}

#[test]
fn fr_121_job_size_is_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Session::new(FakeHost::new(tmp.path()), true);
    let job = begin(&mut s, 30);
    let per = MAX_FILE_BYTES;
    let fits = (MAX_JOB_BYTES / per) as u32;
    for index in 0..=fits {
        let r = send(
            &mut s,
            json!({"v":1,"id":"p","type":"putFile","jobId":job,"index":index,"name":"a.pdf","kind":"pdf",
                   "size":per,"sha256":"0".repeat(64),"chunk":0,"chunks":4,"data":""}),
        );
        if index < fits {
            assert_eq!(r["ok"], true, "{index}: {r}");
        } else {
            assert_eq!(code(&r), "too_large");
        }
    }
}

#[test]
fn fr_120_io_launch_and_internal_errors() {
    // io_error: the data folder is a file, so no inbox can be made.
    let tmp = tempfile::tempdir().unwrap();
    let blocked = tmp.path().join("blocked");
    std::fs::write(&blocked, b"").unwrap();
    let mut s = Session::new(FakeHost::new(&blocked), true);
    let r = send(
        &mut s,
        json!({"v":1,"id":"b","type":"beginJob","source":"pluryfact","label":"x","action":"configure","expectedFiles":1}),
    );
    assert_eq!(code(&r), "io_error");

    // launch_failed: the handoff is removed again and the job stays open,
    // so the extension may retry or abort.
    let tmp = tempfile::tempdir().unwrap();
    let mut host = FakeHost::new(tmp.path());
    host.fail_launch = true;
    let mut s = Session::new(host, true);
    let job = begin(&mut s, 1);
    put_doc(&mut s, &job, 0, "a.pdf", "pdf", PDF, 1);
    let r = send(
        &mut s,
        json!({"v":1,"id":"c","type":"commitJob","jobId":job}),
    );
    assert_eq!(code(&r), "launch_failed");
    assert_eq!(s.open_jobs(), 1);
    assert_eq!(
        std::fs::read_dir(tmp.path().join("handoff"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        code(&send(
            &mut s,
            json!({"v":1,"id":"c","type":"commitJob","jobId":"zzz"})
        )),
        "unknown_job"
    );

    // internal: the host could not enumerate printers.
    let mut host = FakeHost::new(tmp.path());
    host.fail_printers = true;
    let mut s = Session::new(host, true);
    assert_eq!(
        code(&send(&mut s, json!({"v":1,"id":"1","type":"listPrinters"}))),
        "internal"
    );
}

/// Every error code of the protocol is produced by some request above;
/// this table keeps the list and the tests honest about it.
#[test]
fn fr_120_every_error_code_is_reachable() {
    let covered = [
        "unsupported_version",
        "unknown_type",
        "bad_request",
        "origin_not_allowed",
        "too_large",
        "too_many_files",
        "bad_pdf",
        "checksum_mismatch",
        "unknown_job",
        "job_incomplete",
        "io_error",
        "launch_failed",
        "internal",
    ];
    let all: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(all, covered);
}

/// However many printers and paper sizes the spooler reports, the reply
/// stays under the browser's 1 MB limit: the paper lists go first, and a
/// list still too large is refused as `too_large`.
#[test]
fn fr_120_replies_stay_under_one_megabyte() {
    let tmp = tempfile::tempdir().unwrap();
    let printers = |name_len: usize| -> Vec<PrinterEntry> {
        (0..500)
            .map(|i| PrinterEntry {
                id: format!("printer-{i}"),
                name: "n".repeat(name_len),
                is_default: false,
                capabilities: Some(protocol::PrinterCapabilities {
                    duplex: true,
                    color: true,
                    papers: (0..64)
                        .map(|p| format!("paper-{p}-{}", "y".repeat(100)))
                        .collect(),
                }),
            })
            .collect()
    };
    let mut host = FakeHost::new(tmp.path());
    host.printers = printers(50);
    let mut s = Session::new(host, true);
    let r = send(&mut s, json!({"v":1,"id":"1","type":"listPrinters"}));
    assert_eq!(r["ok"], true);
    let list = r["printers"].as_array().unwrap();
    assert_eq!(list.len(), MAX_PRINTERS);
    assert_eq!(
        list[0]["capabilities"]["papers"],
        json!([]),
        "papers dropped"
    );
    assert_eq!(list[0]["capabilities"]["duplex"], true, "the rest kept");

    let mut host = FakeHost::new(tmp.path());
    host.printers = printers(6_000);
    let mut s = Session::new(host, true);
    let raw = s.handle_bytes(br#"{"v":1,"id":"1","type":"listPrinters"}"#);
    assert!(raw.len() <= crate::bridge::framing::MAX_OUTBOUND_BYTES);
    let r: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(code(&r), "too_large");
    assert_eq!(r["id"], "1");
}

#[test]
fn fr_123_overrides_map_onto_the_print_settings() {
    use crate::profiles::ProfileDuplex;
    use crate::queue::job::{ColorMode, Orientation, PaperSize};
    let o = to_selection_overrides(
        protocol::Overrides {
            copies: Some(3),
            duplex: Some(DuplexOverride::ShortEdge),
            color: Some(ColorOverride::Color),
            paper: Some("letter".into()),
            orientation: Some(OrientationOverride::Landscape),
            collate: Some(true),
        },
        Some("Brother".into()),
    )
    .unwrap();
    assert_eq!(o.copies, Some(3));
    assert_eq!(o.duplex, Some(ProfileDuplex::ShortEdge));
    assert_eq!(o.color, Some(ColorMode::Color));
    assert_eq!(o.paper, Some(PaperSize::LETTER));
    assert_eq!(o.orientation, Some(Orientation::Landscape));
    assert_eq!(o.collate, Some(true));
    assert_eq!(o.printer.as_deref(), Some("Brother"));
    // Paper sizes a printer reports pass through as written.
    let o = to_selection_overrides(
        protocol::Overrides {
            paper: Some("A6".into()),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(o.paper.unwrap().as_str(), "A6");
    assert!(to_selection_overrides(Default::default(), None)
        .unwrap()
        .is_empty());
}
