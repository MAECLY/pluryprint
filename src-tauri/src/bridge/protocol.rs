//! Protocol v1 between PluryFact and the bridge: the message types and the
//! request parser.
//!
//! Every message is `{ "v": 1, "id": "<string>", "type": "<type>", … }`. A
//! reply repeats `v`, `id` and `type` and adds `"ok": true` plus the reply's
//! fields, or `"ok": false` and `"error": { "code", "message" }`.
//!
//! The canonical examples of every exchange live in
//! `src-tauri/tests/fixtures/bridge-protocol/` and are copied byte for byte
//! into PluryFact's repository; `tests/bridge_protocol_contract.rs` checks
//! that these types read and write them exactly. Field names are camelCase on
//! the wire. Unknown top-level fields are tolerated (a later minor revision
//! may add optional ones), but unknown *override* fields are refused: a print
//! setting must never be silently ignored.

use serde::{Deserialize, Serialize};

/// The protocol version this host speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest `id` a request may carry, in bytes.
pub const MAX_ID_BYTES: usize = 128;

/// Largest document, decoded (50 MiB). A RIDE is a few tens of KB.
pub const MAX_FILE_BYTES: u64 = 52_428_800;
/// Most documents in one job.
pub const MAX_FILES: u32 = 5_000;
/// Largest decoded chunk of a document (16 MiB; ~22 MiB once base64-encoded,
/// well inside the 64 MiB a message may carry).
pub const MAX_CHUNK_BYTES: u64 = 16_777_216;
/// Largest job, all documents together (1 GiB).
pub const MAX_JOB_BYTES: u64 = 1_073_741_824;

/// The operations this host implements, in the order `hello` lists them.
pub const CAPABILITIES: [&str; 6] = [
    "listPrinters",
    "listProfiles",
    "beginJob",
    "putFile",
    "commitJob",
    "abortJob",
];

/// What a failed request reports. `snake_case` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// `v` is not 1, or `hello` offered no protocol this host speaks.
    UnsupportedVersion,
    /// `type` names no operation this host knows.
    UnknownType,
    /// The message is not valid JSON, misses a field, or a value is invalid
    /// (including an XML document that is not well formed, or text that is
    /// not UTF-8/ISO-8859-1).
    BadRequest,
    /// The caller's origin is not one the host is registered for.
    OriginNotAllowed,
    /// A document, a chunk, a job or a message is over its limit.
    TooLarge,
    /// A job announces or sends more documents than allowed.
    TooManyFiles,
    /// A document of kind `pdf` does not start with `%PDF-`.
    BadPdf,
    /// A completed document's SHA-256 is not the one announced.
    ChecksumMismatch,
    /// `jobId` names no open job of this connection.
    UnknownJob,
    /// `commitJob` before every announced document arrived.
    JobIncomplete,
    /// The host could not write to its inbox or handoff folder.
    IoError,
    /// The handoff was written but the app could not be started.
    LaunchFailed,
    /// Anything else (a bug).
    Internal,
}

impl ErrorCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnsupportedVersion => "unsupported_version",
            ErrorCode::UnknownType => "unknown_type",
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::OriginNotAllowed => "origin_not_allowed",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::TooManyFiles => "too_many_files",
            ErrorCode::BadPdf => "bad_pdf",
            ErrorCode::ChecksumMismatch => "checksum_mismatch",
            ErrorCode::UnknownJob => "unknown_job",
            ErrorCode::JobIncomplete => "job_incomplete",
            ErrorCode::IoError => "io_error",
            ErrorCode::LaunchFailed => "launch_failed",
            ErrorCode::Internal => "internal",
        }
    }

    /// Every code, for exhaustive tests and documentation.
    pub const ALL: [ErrorCode; 13] = [
        ErrorCode::UnsupportedVersion,
        ErrorCode::UnknownType,
        ErrorCode::BadRequest,
        ErrorCode::OriginNotAllowed,
        ErrorCode::TooLarge,
        ErrorCode::TooManyFiles,
        ErrorCode::BadPdf,
        ErrorCode::ChecksumMismatch,
        ErrorCode::UnknownJob,
        ErrorCode::JobIncomplete,
        ErrorCode::IoError,
        ErrorCode::LaunchFailed,
        ErrorCode::Internal,
    ];
}

/// A request that could not be honoured: the code and an English message
/// for the extension's logs (the extension shows its own, translated text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Human-readable detail, in English.
    pub message: String,
}

impl ProtocolError {
    /// Build an error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Shorthand for [`ErrorCode::BadRequest`].
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }
}

/// The part of every message that identifies it. Echoed in the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol version.
    pub v: u32,
    /// Caller-chosen correlation id.
    pub id: String,
    /// Operation name.
    #[serde(rename = "type")]
    pub kind: String,
}

impl Envelope {
    /// The envelope used to answer a message too broken to have one.
    pub fn unknown() -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: String::new(),
            kind: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// `hello`: the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloRequest {
    /// Who is calling (`"pluryfact"`).
    pub client: String,
    /// The caller's version, for the log.
    pub client_version: String,
    /// Protocol versions the caller speaks.
    pub protocols: Vec<u32>,
}

/// What to do with a committed job (the app's context-menu actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobAction {
    /// Open the Print Job screen with the documents loaded; nothing prints
    /// until the user confirms. The default PluryFact uses.
    Configure,
    /// Add to the queue, held, for review in the Queue screen.
    Queue,
    /// Print at once with the chosen (or default) profile.
    PrintDefault,
}

impl JobAction {
    /// The app's equivalent.
    pub fn to_context_action(self) -> crate::cli::args::ContextAction {
        use crate::cli::args::ContextAction;
        match self {
            JobAction::Configure => ContextAction::Configure,
            JobAction::Queue => ContextAction::Queue,
            JobAction::PrintDefault => ContextAction::PrintDefault,
        }
    }
}

/// `overrides.duplex`, in `print --duplex`'s vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DuplexOverride {
    /// One-sided.
    Off,
    /// Two-sided, long edge.
    LongEdge,
    /// Two-sided, short edge.
    ShortEdge,
}

/// `overrides.color`, in `print --color|--mono`'s vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorOverride {
    /// Colour.
    Color,
    /// Grayscale.
    Mono,
}

/// `overrides.orientation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrientationOverride {
    /// Per page, from the source.
    Auto,
    /// Portrait.
    Portrait,
    /// Landscape.
    Landscape,
}

/// `beginJob.overrides`: the same values and checks as `pluryprint print`'s
/// flags (`--copies`, `--duplex`, `--color|--mono`, `--paper`,
/// `--orientation`, `--collate|--no-collate`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overrides {
    /// 1-999.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copies: Option<u16>,
    /// Double-sided mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplex: Option<DuplexOverride>,
    /// Colour mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorOverride>,
    /// Paper: `a4`, `letter`, `legal`, `a3`, `auto` or a size the printer
    /// reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
    /// Orientation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<OrientationOverride>,
    /// Collate copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collate: Option<bool>,
}

/// `beginJob`: open a job and announce how many documents follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginJobRequest {
    /// Must be `"pluryfact"`.
    pub source: String,
    /// Display label, at most 120 characters.
    pub label: String,
    /// What the app does once the job is committed.
    pub action: JobAction,
    /// Profile id (from `listProfiles`), or none for the default profile.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Printer id (from `listPrinters`), or none for the profile's.
    #[serde(default)]
    pub printer_id: Option<String>,
    /// Settings on top of the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<Overrides>,
    /// How many documents `putFile` will send (indexes `0..expectedFiles`).
    pub expected_files: u32,
}

/// The kind of a document, which fixes its extension and its validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// A RIDE (or any PDF): must start with `%PDF-`.
    Pdf,
    /// An electronic voucher: must be well-formed XML, no DTD.
    Xml,
    /// PluryFact's TXT report: UTF-8 or ISO-8859-1 text.
    Txt,
}

impl FileKind {
    /// The extension the stored file is given, whatever the sent name said.
    pub fn extension(self) -> &'static str {
        match self {
            FileKind::Pdf => "pdf",
            FileKind::Xml => "xml",
            FileKind::Txt => "txt",
        }
    }
}

/// `putFile`: one chunk of one document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutFileRequest {
    /// From `beginJob`.
    pub job_id: String,
    /// Position in the print order, `0..expectedFiles`.
    pub index: u32,
    /// The document's name. Only a sanitized base name of it is used.
    pub name: String,
    /// What the document is.
    pub kind: FileKind,
    /// The whole document's size in bytes.
    pub size: u64,
    /// The whole document's SHA-256, lowercase hex.
    pub sha256: String,
    /// This chunk's number, `0..chunks`, sent in order.
    pub chunk: u32,
    /// How many chunks the document is split into.
    pub chunks: u32,
    /// This chunk's bytes, standard base64 with padding.
    pub data: String,
}

/// `commitJob` / `abortJob`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    /// From `beginJob`.
    pub job_id: String,
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// `hello`.
    Hello(HelloRequest),
    /// `listPrinters`.
    ListPrinters,
    /// `listProfiles`.
    ListProfiles,
    /// `beginJob`.
    BeginJob(BeginJobRequest),
    /// `putFile`.
    PutFile(PutFileRequest),
    /// `commitJob`.
    CommitJob(JobRef),
    /// `abortJob`.
    AbortJob(JobRef),
}

/// Parse one message body into its envelope and request.
///
/// On failure the envelope is returned whenever it could be read, so the
/// error reply still carries the caller's `id` and `type`.
pub fn parse_request(body: &[u8]) -> Result<(Envelope, Request), (Envelope, ProtocolError)> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        (
            Envelope::unknown(),
            ProtocolError::bad_request("the message is not valid JSON"),
        )
    })?;
    let Some(obj) = value.as_object() else {
        return Err((
            Envelope::unknown(),
            ProtocolError::bad_request("the message is not a JSON object"),
        ));
    };
    let id = match obj.get("id") {
        Some(serde_json::Value::String(id)) if id.len() <= MAX_ID_BYTES => id.clone(),
        _ => {
            return Err((
                Envelope::unknown(),
                ProtocolError::bad_request(format!(
                    "id must be a string of at most {MAX_ID_BYTES} bytes"
                )),
            ))
        }
    };
    let kind = match obj.get("type") {
        Some(serde_json::Value::String(t)) if t.len() <= 64 => t.clone(),
        _ => {
            let env = Envelope {
                v: PROTOCOL_VERSION,
                id,
                kind: String::new(),
            };
            return Err((env, ProtocolError::bad_request("type must be a string")));
        }
    };
    let env = Envelope {
        v: PROTOCOL_VERSION,
        id,
        kind,
    };
    match obj.get("v").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        _ => {
            return Err((
                env,
                ProtocolError::new(
                    ErrorCode::UnsupportedVersion,
                    format!("this host speaks protocol version {PROTOCOL_VERSION}"),
                ),
            ))
        }
    }

    fn typed<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, ProtocolError> {
        serde_json::from_value(value).map_err(|e| ProtocolError::bad_request(field_error(&e)))
    }

    let request = match env.kind.as_str() {
        "hello" => typed(value).map(Request::Hello),
        "listPrinters" => Ok(Request::ListPrinters),
        "listProfiles" => Ok(Request::ListProfiles),
        "beginJob" => typed(value).map(Request::BeginJob),
        "putFile" => typed(value).map(Request::PutFile),
        "commitJob" => typed(value).map(Request::CommitJob),
        "abortJob" => typed(value).map(Request::AbortJob),
        other => Err(ProtocolError::new(
            ErrorCode::UnknownType,
            format!("unknown message type '{}'", truncate(other, 64)),
        )),
    };
    match request {
        Ok(r) => Ok((env, r)),
        Err(e) => Err((env, e)),
    }
}

/// A serde error as a message that names the problem but never echoes a
/// value back (a value could be a whole base64 chunk).
fn field_error(e: &serde_json::Error) -> String {
    let text = e.to_string();
    // serde_json appends " at line L column C"; keep the description part,
    // capped, so a huge invalid value cannot inflate the reply.
    let text = text.split(" at line ").next().unwrap_or(&text);
    format!("invalid request: {}", truncate(text, 200))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Replies
// ---------------------------------------------------------------------------

/// The limits `hello` advertises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// [`MAX_FILE_BYTES`].
    pub max_file_bytes: u64,
    /// [`MAX_FILES`].
    pub max_files: u32,
    /// [`MAX_CHUNK_BYTES`].
    pub max_chunk_bytes: u64,
    /// [`MAX_JOB_BYTES`].
    pub max_job_bytes: u64,
}

impl Limits {
    /// This host's limits.
    pub fn current() -> Self {
        Self {
            max_file_bytes: MAX_FILE_BYTES,
            max_files: MAX_FILES,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_job_bytes: MAX_JOB_BYTES,
        }
    }
}

/// `hello`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    /// Always `"pluryprint"`.
    pub host: String,
    /// The app's version.
    pub host_version: String,
    /// The negotiated protocol version.
    pub protocol: u32,
    /// [`CAPABILITIES`].
    pub capabilities: Vec<String>,
    /// [`Limits::current`].
    pub limits: Limits,
}

/// What a printer can do, as far as the spooler says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterCapabilities {
    /// Two-sided printing.
    pub duplex: bool,
    /// Colour.
    pub color: bool,
    /// Paper sizes the printer reports (at most 64).
    pub papers: Vec<String>,
}

/// One printer in `listPrinters`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterEntry {
    /// The spooler's id (what `printerId` takes).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The OS default printer.
    pub is_default: bool,
    /// Capabilities, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PrinterCapabilities>,
}

/// `listPrinters`' reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPrintersReply {
    /// The printers.
    pub printers: Vec<PrinterEntry>,
}

/// One profile in `listProfiles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntry {
    /// Profile id (what `profileId` takes).
    pub id: String,
    /// Display name, in the app's language for the built-ins.
    pub name: String,
    /// The app's default profile.
    pub is_default: bool,
}

/// `listProfiles`' reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListProfilesReply {
    /// The profiles, built-ins first.
    pub profiles: Vec<ProfileEntry>,
}

/// `beginJob`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginJobReply {
    /// The id every later message of this job carries.
    pub job_id: String,
}

/// `putFile`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutFileReply {
    /// Bytes of this document received so far.
    pub received: u64,
    /// Whether the document is complete and verified.
    pub complete: bool,
}

/// `commitJob`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitJobReply {
    /// Always `"handed-off"`: the app has the documents.
    pub state: String,
    /// How many documents were handed off.
    pub files: u32,
}

/// `abortJob`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortJobReply {
    /// Always `"aborted"`.
    pub state: String,
}

/// A successful reply: the envelope, `ok: true`, and the body's fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkReply<T> {
    /// Echoed.
    pub v: u32,
    /// Echoed.
    pub id: String,
    /// Echoed.
    #[serde(rename = "type")]
    pub kind: String,
    /// `true`.
    pub ok: bool,
    /// The operation's fields.
    #[serde(flatten)]
    pub body: T,
}

/// A failed reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReply {
    /// Echoed.
    pub v: u32,
    /// Echoed.
    pub id: String,
    /// Echoed.
    #[serde(rename = "type")]
    pub kind: String,
    /// `false`.
    pub ok: bool,
    /// What went wrong.
    pub error: ProtocolError,
}

/// Serialize a successful reply.
pub fn ok_reply<T: Serialize>(env: &Envelope, body: T) -> Vec<u8> {
    serde_json::to_vec(&OkReply {
        v: PROTOCOL_VERSION,
        id: env.id.clone(),
        kind: env.kind.clone(),
        ok: true,
        body,
    })
    .unwrap_or_else(|_| error_reply(env, &ProtocolError::new(ErrorCode::Internal, "encode")))
}

/// Serialize a failed reply.
pub fn error_reply(env: &Envelope, error: &ProtocolError) -> Vec<u8> {
    serde_json::to_vec(&ErrorReply {
        v: PROTOCOL_VERSION,
        id: env.id.clone(),
        kind: env.kind.clone(),
        ok: false,
        error: error.clone(),
    })
    .expect("an error reply always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<(Envelope, Request), (Envelope, ProtocolError)> {
        parse_request(json.as_bytes())
    }

    #[test]
    fn fr_120_parses_every_request_type() {
        let (env, r) =
            parse(r#"{"v":1,"id":"1","type":"hello","client":"pluryfact","clientVersion":"0.2.0","protocols":[1]}"#)
                .unwrap();
        assert_eq!(env.id, "1");
        assert_eq!(env.kind, "hello");
        assert!(matches!(r, Request::Hello(h) if h.protocols == vec![1]));
        assert_eq!(
            parse(r#"{"v":1,"id":"2","type":"listPrinters"}"#)
                .unwrap()
                .1,
            Request::ListPrinters
        );
        assert_eq!(
            parse(r#"{"v":1,"id":"3","type":"listProfiles"}"#)
                .unwrap()
                .1,
            Request::ListProfiles
        );
        let (_, r) = parse(
            r#"{"v":1,"id":"4","type":"beginJob","source":"pluryfact","label":"x","action":"print-default",
                "profileId":null,"printerId":"HP","overrides":{"copies":2,"color":"mono"},"expectedFiles":3}"#,
        )
        .unwrap();
        match r {
            Request::BeginJob(b) => {
                assert_eq!(b.action, JobAction::PrintDefault);
                assert_eq!(b.printer_id.as_deref(), Some("HP"));
                assert_eq!(b.overrides.unwrap().color, Some(ColorOverride::Mono));
                assert_eq!(b.expected_files, 3);
            }
            other => panic!("{other:?}"),
        }
        let (_, r) = parse(
            r#"{"v":1,"id":"5","type":"putFile","jobId":"j","index":0,"name":"a.pdf","kind":"pdf",
                "size":5,"sha256":"00","chunk":0,"chunks":1,"data":"JVBERi0="}"#,
        )
        .unwrap();
        assert!(matches!(r, Request::PutFile(p) if p.kind == FileKind::Pdf));
        assert!(matches!(
            parse(r#"{"v":1,"id":"6","type":"commitJob","jobId":"j"}"#)
                .unwrap()
                .1,
            Request::CommitJob(_)
        ));
        assert!(matches!(
            parse(r#"{"v":1,"id":"7","type":"abortJob","jobId":"j"}"#)
                .unwrap()
                .1,
            Request::AbortJob(_)
        ));
    }

    #[test]
    fn fr_120_each_protocol_error_is_reported_with_the_callers_envelope() {
        // Not JSON / not an object / no id: no envelope to echo.
        for raw in [
            "not json",
            "[1,2]",
            r#"{"v":1,"type":"hello"}"#,
            r#"{"v":1,"id":7,"type":"hello"}"#,
        ] {
            let (env, e) = parse(raw).unwrap_err();
            assert_eq!(env, Envelope::unknown(), "{raw}");
            assert_eq!(e.code, ErrorCode::BadRequest, "{raw}");
        }
        // An over-long id is refused rather than echoed.
        let long = format!(
            r#"{{"v":1,"id":"{}","type":"hello"}}"#,
            "x".repeat(MAX_ID_BYTES + 1)
        );
        assert_eq!(parse(&long).unwrap_err().1.code, ErrorCode::BadRequest);

        let (env, e) = parse(r#"{"v":2,"id":"9","type":"hello"}"#).unwrap_err();
        assert_eq!((env.id.as_str(), env.kind.as_str()), ("9", "hello"));
        assert_eq!(e.code, ErrorCode::UnsupportedVersion);
        assert_eq!(
            parse(r#"{"id":"9","type":"hello"}"#).unwrap_err().1.code,
            ErrorCode::UnsupportedVersion
        );

        let (env, e) = parse(r#"{"v":1,"id":"9","type":"printNow"}"#).unwrap_err();
        assert_eq!(env.kind, "printNow");
        assert_eq!(e.code, ErrorCode::UnknownType);

        // A missing field, a wrong type and a bad enum value are bad requests.
        for raw in [
            r#"{"v":1,"id":"9","type":"hello","client":"pluryfact"}"#,
            r#"{"v":1,"id":"9","type":"beginJob","source":"pluryfact","label":"x","action":"burn","expectedFiles":1}"#,
            r#"{"v":1,"id":"9","type":"putFile","jobId":"j","index":-1,"name":"a","kind":"pdf","size":1,"sha256":"","chunk":0,"chunks":1,"data":""}"#,
            r#"{"v":1,"id":"9","type":"putFile","jobId":"j","index":0,"name":"a","kind":"zip","size":1,"sha256":"","chunk":0,"chunks":1,"data":""}"#,
            r#"{"v":1,"id":"9","type":"commitJob"}"#,
        ] {
            let (env, e) = parse(raw).unwrap_err();
            assert_eq!(env.id, "9");
            assert_eq!(e.code, ErrorCode::BadRequest, "{raw}");
        }
    }

    /// An override the host does not know is refused, never ignored.
    #[test]
    fn fr_123_unknown_overrides_are_refused() {
        let raw = r#"{"v":1,"id":"1","type":"beginJob","source":"pluryfact","label":"x","action":"configure",
                      "overrides":{"scale":"fit"},"expectedFiles":1}"#;
        assert_eq!(parse(raw).unwrap_err().1.code, ErrorCode::BadRequest);
        // …while an unknown top-level field is tolerated (forward compatible).
        let raw = r#"{"v":1,"id":"1","type":"listPrinters","traceId":"abc"}"#;
        assert!(parse(raw).is_ok());
    }

    /// An error message never echoes a value back: a bad field can be a
    /// whole base64 chunk, and the reply must stay small.
    #[test]
    fn fr_120_error_messages_do_not_echo_values() {
        let huge = "A".repeat(100_000);
        let raw = format!(
            r#"{{"v":1,"id":"1","type":"putFile","jobId":"j","index":0,"name":"a","kind":"{huge}","size":1,"sha256":"","chunk":0,"chunks":1,"data":""}}"#
        );
        let (_, e) = parse(&raw).unwrap_err();
        assert!(e.message.len() < 400, "{}", e.message.len());
    }

    #[test]
    fn fr_120_error_codes_spell_snake_case() {
        for code in ErrorCode::ALL {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_str()));
        }
    }

    #[test]
    fn fr_120_replies_echo_the_envelope() {
        let env = Envelope {
            v: 1,
            id: "42".into(),
            kind: "commitJob".into(),
        };
        let ok: serde_json::Value = serde_json::from_slice(&ok_reply(
            &env,
            CommitJobReply {
                state: "handed-off".into(),
                files: 3,
            },
        ))
        .unwrap();
        assert_eq!(
            ok,
            serde_json::json!({"v":1,"id":"42","type":"commitJob","ok":true,"state":"handed-off","files":3})
        );
        let err: serde_json::Value = serde_json::from_slice(&error_reply(
            &env,
            &ProtocolError::new(ErrorCode::JobIncomplete, "2 of 3"),
        ))
        .unwrap();
        assert_eq!(
            err,
            serde_json::json!({"v":1,"id":"42","type":"commitJob","ok":false,
                               "error":{"code":"job_incomplete","message":"2 of 3"}})
        );
    }
}
