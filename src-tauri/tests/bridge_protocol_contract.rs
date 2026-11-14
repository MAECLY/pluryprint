//! FR-120 — the bridge protocol's contract, pinned on the canonical fixtures
//! in `tests/fixtures/bridge-protocol/` (copied byte for byte into
//! PluryFact's repository, which checks its own types against them).
//!
//! - every reply fixture reads into the host's typed reply and writes back
//!   to exactly the same JSON (no field missing, renamed or extra);
//! - every request fixture is accepted by the host's parser;
//! - the exchanges replay against the real state machine (a fake machine
//!   behind it) and produce the fixtures' replies;
//! - there is one error fixture per error code, and no other.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use pluryprint_lib::bridge::protocol::{
    self, AbortJobReply, BeginJobReply, CommitJobReply, ErrorCode, ErrorReply, HelloReply,
    ListPrintersReply, ListProfilesReply, OkReply, PrinterEntry, ProfileEntry, PutFileReply,
    Request,
};
use pluryprint_lib::bridge::session::{Host, Session};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bridge-protocol")
}

fn read(name: &str) -> Value {
    let p = dir().join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn names() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    out.sort();
    out
}

/// Read a fixture into `T` and write it back: the result must be the same
/// JSON value.
fn round_trip<T: DeserializeOwned + Serialize>(name: &str) {
    let value = read(name);
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let back = serde_json::to_value(&typed).unwrap();
    assert_eq!(back, value, "{name} does not round-trip");
}

#[test]
fn fr_120_every_reply_fixture_round_trips_through_the_host_types() {
    let mut seen = 0;
    for name in names().iter().filter(|n| n.ends_with(".response.json")) {
        let ty = read(name)["type"].as_str().unwrap_or_default().to_string();
        if name.starts_with("error.") {
            round_trip::<ErrorReply>(name);
        } else {
            match ty.as_str() {
                "hello" => round_trip::<OkReply<HelloReply>>(name),
                "listPrinters" => round_trip::<OkReply<ListPrintersReply>>(name),
                "listProfiles" => round_trip::<OkReply<ListProfilesReply>>(name),
                "beginJob" => round_trip::<OkReply<BeginJobReply>>(name),
                "putFile" => round_trip::<OkReply<PutFileReply>>(name),
                "commitJob" => round_trip::<OkReply<CommitJobReply>>(name),
                "abortJob" => round_trip::<OkReply<AbortJobReply>>(name),
                other => panic!("{name}: unexpected type {other}"),
            }
        }
        seen += 1;
    }
    assert!(seen >= 24, "only {seen} reply fixtures");
}

#[test]
fn fr_120_every_request_fixture_round_trips_and_parses() {
    use protocol::{BeginJobRequest, HelloRequest, JobRef, PutFileRequest};
    for name in names().iter().filter(|n| n.ends_with(".request.json")) {
        let value = read(name);
        let bytes = serde_json::to_vec(&value).unwrap();
        if name.starts_with("error.") {
            // Its reply says what the host makes of it; checked by replay.
            continue;
        }
        let (env, req) = protocol::parse_request(&bytes)
            .unwrap_or_else(|(_, e)| panic!("{name} refused: {e:?}"));
        assert_eq!(env.v, 1);
        // The typed request writes back to the same fields (envelope apart).
        let mut fields = value.as_object().unwrap().clone();
        for k in ["v", "id", "type"] {
            fields.remove(k);
        }
        let back = match req {
            Request::Hello(r) => serde_json::to_value::<HelloRequest>(r),
            Request::BeginJob(r) => serde_json::to_value::<BeginJobRequest>(r),
            Request::PutFile(r) => serde_json::to_value::<PutFileRequest>(r),
            Request::CommitJob(r) | Request::AbortJob(r) => serde_json::to_value::<JobRef>(r),
            Request::ListPrinters | Request::ListProfiles => Ok(Value::Object(Default::default())),
        }
        .unwrap();
        assert_eq!(back, Value::Object(fields), "{name}");
    }
}

#[test]
fn fr_120_there_is_one_error_fixture_per_error_code() {
    let fixtures: BTreeSet<String> = names()
        .iter()
        .filter_map(|n| {
            n.strip_prefix("error.")?
                .strip_suffix(".response.json")
                .map(String::from)
        })
        .collect();
    let codes: BTreeSet<String> = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    assert_eq!(fixtures, codes);
    for code in &codes {
        let reply = read(&format!("error.{code}.response.json"));
        assert_eq!(reply["error"]["code"], code.as_str());
        assert_eq!(reply["ok"], false);
    }
}

/// A machine whose printers and profiles are the fixtures' and whose
/// "launch" is recorded.
struct FixtureHost {
    data: PathBuf,
    launched: RefCell<u32>,
}

impl Host for FixtureHost {
    fn printers(&self) -> Result<Vec<PrinterEntry>, String> {
        Ok(serde_json::from_value(read("listPrinters.response.json")["printers"].clone()).unwrap())
    }
    fn profiles(&self) -> Result<Vec<ProfileEntry>, String> {
        Ok(serde_json::from_value(read("listProfiles.response.json")["profiles"].clone()).unwrap())
    }
    fn data_dir(&self) -> PathBuf {
        self.data.clone()
    }
    fn launch(&self, _args: &[String]) -> Result<(), String> {
        *self.launched.borrow_mut() += 1;
        Ok(())
    }
}

/// Replay `steps` (request, expected reply) in one session. Job ids are the
/// host's own, so the fixture's id is swapped for the real one both ways;
/// the host version is the build's.
fn replay(data: &Path, origin_ok: bool, steps: &[(&str, &str)]) -> Session<FixtureHost> {
    let mut s = Session::new(
        FixtureHost {
            data: data.to_path_buf(),
            launched: RefCell::new(0),
        },
        origin_ok,
    );
    let mut ids: Vec<(String, String)> = Vec::new();
    for (req, want) in steps {
        let mut request = read(req);
        if let Some(fixture_id) = request.get("jobId").and_then(Value::as_str) {
            if let Some((_, real)) = ids.iter().find(|(f, _)| f == fixture_id) {
                request["jobId"] = Value::String(real.clone());
            }
        }
        let raw = s.handle_bytes(&serde_json::to_vec(&request).unwrap());
        let mut got: Value = serde_json::from_slice(&raw).unwrap();
        let mut want = read(want);
        if let (Some(real), Some(fixture)) = (got.get("jobId"), want.get("jobId")) {
            ids.push((
                fixture.as_str().unwrap().into(),
                real.as_str().unwrap().into(),
            ));
            got["jobId"] = fixture.clone();
        }
        if want.get("hostVersion").is_some() {
            assert_eq!(got["hostVersion"], env!("CARGO_PKG_VERSION"));
            got["hostVersion"] = want["hostVersion"].clone();
        }
        if want["ok"] == false {
            // Messages are informational; the code and envelope are the contract.
            assert_eq!(got["error"]["code"], want["error"]["code"], "{req}: {got}");
            got["error"]["message"] = Value::Null;
            want["error"]["message"] = Value::Null;
        }
        assert_eq!(got, want, "{req}");
    }
    s
}

#[test]
fn fr_120_the_exchanges_replay_against_the_host() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = replay(
        tmp.path(),
        true,
        &[
            ("hello.request.json", "hello.response.json"),
            ("listPrinters.request.json", "listPrinters.response.json"),
            ("listProfiles.request.json", "listProfiles.response.json"),
            ("beginJob.request.json", "beginJob.response.json"),
            ("putFile.request.json", "putFile.response.json"),
            ("putFile.xml.request.json", "putFile.xml.response.json"),
            ("putFile.txt.request.json", "putFile.txt.response.json"),
            ("commitJob.request.json", "commitJob.response.json"),
            (
                "beginJob.queue.request.json",
                "beginJob.queue.response.json",
            ),
            (
                "putFile.chunk-0.request.json",
                "putFile.chunk-0.response.json",
            ),
            ("abortJob.request.json", "abortJob.response.json"),
            (
                "error.unsupported_version.request.json",
                "error.unsupported_version.response.json",
            ),
            (
                "error.unknown_type.request.json",
                "error.unknown_type.response.json",
            ),
            (
                "error.bad_request.request.json",
                "error.bad_request.response.json",
            ),
            (
                "error.too_many_files.request.json",
                "error.too_many_files.response.json",
            ),
            (
                "error.unknown_job.request.json",
                "error.unknown_job.response.json",
            ),
        ],
    );
    s.close();
    // The committed job's documents and handoff are on disk; the aborted
    // one left nothing.
    let inbox: Vec<_> = std::fs::read_dir(tmp.path().join("inbox"))
        .unwrap()
        .collect();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        std::fs::read_dir(tmp.path().join("handoff"))
            .unwrap()
            .count(),
        1
    );

    // An extension the host does not know gets `origin_not_allowed`.
    replay(
        tmp.path(),
        false,
        &[(
            "error.origin_not_allowed.request.json",
            "error.origin_not_allowed.response.json",
        )],
    );
}

/// The fixtures' documents are fictitious: no real RUC or access key.
#[test]
fn fr_120_fixtures_carry_no_real_identifiers() {
    for name in names() {
        let text = std::fs::read_to_string(dir().join(&name)).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        if let Some(data) = value.get("data").and_then(Value::as_str) {
            use base64::Engine as _;
            let doc = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            let doc = String::from_utf8_lossy(&doc);
            for ruc in find_rucs(&doc) {
                assert_eq!(ruc, "0999999999001", "{name}");
            }
        }
        assert!(
            text.ends_with("}\n"),
            "{name}: pretty JSON with a final newline"
        );
    }
}

fn find_rucs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<ruc>") {
        rest = &rest[i + 5..];
        if let Some(j) = rest.find("</ruc>") {
            out.push(rest[..j].to_string());
        }
    }
    out
}
