//! HEIC/HEIF → PDF through a decoder the system already has (FR-052).
//!
//! The `image` crate has no HEIF decoder at any feature combination, and
//! bundling `libheif` would drag a native build (and HEVC patent questions)
//! into the app. macOS and Windows ship a decoder, so we use theirs, and on
//! Linux we use a program the distribution packages:
//!
//! - **macOS**: `sips -s format png <in> --out <work>/heif.png` (part of the
//!   base system, `/usr/bin/sips`), then the regular image → PDF path.
//! - **Windows**: Windows Imaging Component. WIC decodes HEIF only when the
//!   Microsoft Store *HEIF Image Extensions* (and, for HEIC, *HEVC Video
//!   Extensions*) are installed; without them the decoder lookup fails and the
//!   job pauses with a [`EngineError::DependencyMissing`] that names them.
//! - **Linux**: an external decoder found like any other dependency
//!   ([`crate::engine::deps::Dependency::HeifDecoder`]): libheif's
//!   `heif-dec`/`heif-convert` (`libheif-examples` on Debian and Ubuntu,
//!   `libheif-tools` on Fedora) or ImageMagick's `magick`/`convert`, which
//!   decode through libheif as well. It writes a PNG into the work directory
//!   ([`convert_with_external`]), bounded and cancellable like every
//!   converter process; without one the job pauses with the
//!   `heif-decoder` remediation.
//!
//! On Linux the decoder is a program the app did not choose and cannot pin,
//! so what reaches it is narrowed twice ([`convert_with_external`]): the
//! file is copied into the private conversion folder and that copy must
//! start with an ISO-BMFF `ftyp` box naming a HEIF brand ([`header`]) before
//! any process starts, and ImageMagick runs under a security policy that
//! allows nothing but reading HEIC/HEIF and writing PNG
//! ([`imagemagick_policy_env`]). macOS and Windows decode with the system's
//! own HEIF reader and keep their behaviour.

pub mod header;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::WorkDir;
use crate::engine::{EngineError, Failure};

/// HEIF-family extensions.
pub const HEIF_EXTS: &[&str] = &["heic", "heif"];

/// FR-058 message for a Windows machine without the HEIF codec.
pub const WINDOWS_CODEC_MISSING: &str = "The HEIF image decoder is missing — HEIC/HEIF photos \
cannot be printed until it is installed. Install \"HEIF Image Extensions\" (and \"HEVC Video \
Extensions\" for .heic) from the Microsoft Store, then retry the job";

/// FR-058 message for a Windows machine that has the HEIF container codec but
/// no HEVC video decoder: WIC finds a decoder for the file and then fails to
/// decode its HEVC-compressed pixels.
pub const WINDOWS_HEVC_MISSING: &str = "The HEVC video decoder this HEIC photo needs is \
missing — install \"HEVC Video Extensions\" from the Microsoft Store, then retry the job";

/// FR-058 message on an OS with no system HEIF decoder. No build emits it
/// any more (Linux looks for an external decoder); the reason stays valid for
/// a `queue.json` that still names it.
pub const NO_SYSTEM_DECODER: &str = "No HEIF image decoder is available on this system — \
convert the photo to JPEG or PNG first";

/// The name the Windows decoder goes by in messages (`{{tool}}`): a product
/// name, so it reads the same in every language.
pub const WIC_LABEL: &str = "Windows Imaging Component";

/// The `sips` argument vector converting `input` to the PNG `png`. Pure.
pub fn sips_args(input: &Path, png: &Path) -> Vec<String> {
    vec![
        "-s".to_string(),
        "format".to_string(),
        "png".to_string(),
        input.to_string_lossy().into_owned(),
        "--out".to_string(),
        png.to_string_lossy().into_owned(),
    ]
}

/// The kind of external HEIF decoder a program is, by its name (Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalDecoder {
    /// libheif's `heif-dec` (1.17+) or `heif-convert`: `<in> <out>`.
    Libheif,
    /// ImageMagick's `magick` (7) or `convert` (6).
    ImageMagick,
}

impl ExternalDecoder {
    /// Which decoder `program` is, from its file name; `None` for a program
    /// that is neither. Pure.
    pub fn of(program: &Path) -> Option<Self> {
        match program.file_name()?.to_str()? {
            "heif-dec" | "heif-convert" => Some(Self::Libheif),
            "magick" | "convert" => Some(Self::ImageMagick),
            _ => None,
        }
    }

    /// The name used in messages and as the converter's label (`{{tool}}`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Libheif => "libheif",
            Self::ImageMagick => "ImageMagick",
        }
    }

    /// The argument vector converting `input` to the PNG `png`. Never a shell
    /// string. Pure.
    ///
    /// Both paths are absolute (the engine only receives canonical paths), so
    /// neither can be read as an option. libheif 1.12, still the version on
    /// Ubuntu 22.04, takes no `--`. ImageMagick decodes only the first image
    /// of the file (`[0]`) with its HEIC coder (`heic:`, which reads every
    /// HEIF file through libheif) and is told the output format explicitly
    /// (`png:`). Without the coder prefix it would pick a decoder from the
    /// file's content, so a hostile `.heic` that is really an SVG, a
    /// PostScript file or anything else would reach that coder or its
    /// delegate program. ImageMagick also expands `*`, `?` and `[…]` in a
    /// file name; both decoders are only ever given the copy
    /// [`stage_input`] makes under a fixed name. Neither decoder is asked to
    /// rotate: libheif already applies the HEIF orientation while decoding,
    /// and rotating again by the EXIF tag would turn the photo twice.
    pub fn args(self, input: &Path, png: &Path) -> Vec<String> {
        let input = input.to_string_lossy().into_owned();
        let png = png.to_string_lossy().into_owned();
        match self {
            Self::Libheif => vec![input, png],
            Self::ImageMagick => vec![format!("heic:{input}[0]"), format!("png:{png}")],
        }
    }
}

/// The name of the private copy every external decoder is given.
const STAGED_NAME: &str = "input.heic";

/// Copy `input` into `work` (mode 0700, this conversion's own) as
/// [`STAGED_NAME`] and return the copy's path.
///
/// Everything after this reads the copy, never `input` again: the header
/// check and the decoder see the same bytes, so a file swapped or rewritten
/// at the original path after the check cannot reach a decoder. Only a
/// regular file is copied ([`header::open_regular`]): a FIFO or a device
/// named `.heic` would block the copy or never end it. A file that cannot
/// be read, or is not a regular file, is `cannot_read_input`; a copy that
/// cannot be written is `workdir_failed`.
fn stage_input(input: &Path, work: &WorkDir) -> Result<PathBuf, EngineError> {
    let staged = work.path().join(STAGED_NAME);
    let mut source = header::open_regular(input).map_err(|e| header::unreadable(input, &e))?;
    let workdir_failed = |e: std::io::Error| {
        EngineError::ConversionFailed(Failure::new(
            "workdir_failed",
            format!("cannot prepare the conversion folder: {e}"),
        ))
    };
    let mut copy = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(workdir_failed)?;
    std::io::copy(&mut source, &mut copy).map_err(workdir_failed)?;
    Ok(staged)
}

/// The PNG an external decoder wrote for `png`. libheif writes one file per
/// top-level image and numbers them (`heif-1.png`, `heif-2.png`…) when the
/// file holds more than one; the first is the photo. `None` when it wrote
/// nothing.
fn decoded_png(png: &Path) -> Option<PathBuf> {
    if png.is_file() {
        return Some(png.to_path_buf());
    }
    let stem = png.file_stem()?.to_str()?;
    let first = png.with_file_name(format!("{stem}-1.png"));
    first.is_file().then_some(first)
}

/// The ImageMagick security policy for HEIF decoding (`heif/magick-policy.xml`):
/// no delegate program, no filter, no `@file` indirection, only the HEIC and
/// PNG modules, read HEIC/HEIF and write PNG, and Ubuntu's resource limits.
pub const MAGICK_POLICY: &str = include_str!("heif/magick-policy.xml");

/// Write [`MAGICK_POLICY`] into `work` and return the environment that makes
/// ImageMagick use it.
///
/// ImageMagick 6 and 7 both look for `policy.xml` in the folders
/// `MAGICK_CONFIGURE_PATH` lists before their own, and both apply the last
/// policy that matches a coder, module or path, which is why the policy
/// closes each domain before it opens the one use. The folder is the
/// conversion's private one (mode 0700, removed with it), so nothing else
/// can swap the file between being written and being read. Pixels that do
/// not fit the memory limit spill to `MAGICK_TEMPORARY_PATH`, which is
/// pointed there too instead of the shared `/tmp`.
pub fn imagemagick_policy_env(
    work: &WorkDir,
) -> Result<Vec<(&'static str, OsString)>, EngineError> {
    let config = work.path().join("magick-policy");
    let temp = work.path().join("magick-tmp");
    std::fs::create_dir(&config)
        .and_then(|()| std::fs::write(config.join("policy.xml"), MAGICK_POLICY))
        .and_then(|()| std::fs::create_dir(&temp))
        .map_err(|e| {
            EngineError::ConversionFailed(Failure::new(
                "workdir_failed",
                format!("cannot prepare the conversion folder: {e}"),
            ))
        })?;
    Ok(vec![
        ("MAGICK_CONFIGURE_PATH", config.into_os_string()),
        ("MAGICK_TEMPORARY_PATH", temp.into_os_string()),
    ])
}

/// The memory an external HEIF decoder may use (resident set, sampled by
/// [`super::process::run_capped`]): a hostile file can claim huge
/// dimensions, and the decoder runs outside the in-process image limits.
/// Generous enough for a 100-megapixel photo through ImageMagick's 16-bit
/// pixel cache.
pub const EXTERNAL_DECODER_MEMORY_CAP: u64 = 2 * 1024 * 1024 * 1024;

/// Convert `input` with the external decoder at `decoder` (Linux), writing
/// the PDF to `output`, once its header says it is a HEIF file. The only
/// way a decoder is started.
///
/// `input` is first copied into the private conversion folder
/// ([`stage_input`]); the copy's header is checked ([`header::check_as`], a
/// bounded read) and only the copy is handed to the decoder, so what was
/// checked is what is decoded. A file that is not HEIF whatever its name
/// (an SVG, an MVG or PostScript program, a PBM, an empty or truncated
/// file) is `heif_decode_failed`, its copy is removed and no decoder process
/// is started. ImageMagick runs under [`MAGICK_POLICY`].
///
/// A decoder that disappeared between being found and being started pauses
/// the job like a missing one (`heif-decoder`); one that runs and writes no
/// image, or needs more than [`EXTERNAL_DECODER_MEMORY_CAP`], could not read
/// the file (`heif_decode_failed`).
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
pub(super) fn convert_with_external(
    decoder: &Path,
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), EngineError> {
    let staged = stage_input(input, work)?;
    if let Err(e) = header::check_as(&staged, input) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    // Anything that is not a known decoder is treated as libheif's
    // interface, the simplest one.
    let kind = ExternalDecoder::of(decoder).unwrap_or(ExternalDecoder::Libheif);
    let png = work.path().join("heif.png");
    let mut cmd = crate::util::appimage::host_command(decoder);
    cmd.args(kind.args(&staged, &png)).current_dir(work.path());
    if kind == ExternalDecoder::ImageMagick {
        cmd.envs(imagemagick_policy_env(work)?);
    }
    let log = work.path().join("heif-decoder.log");
    progress(0.0);
    let ran = super::process::run_capped(
        cmd,
        kind.label(),
        cancel,
        timeout,
        &log,
        EXTERNAL_DECODER_MEMORY_CAP,
        super::process::CapScope::Process,
    );
    match ran {
        Ok(()) => {}
        Err(super::process::CappedError::OverMemory) => {
            return Err(EngineError::ConversionFailed(
                Failure::new(
                    "heif_decode_failed",
                    format!(
                        "{} needed more than {} MB of memory for this HEIC/HEIF file and was stopped",
                        kind.label(),
                        EXTERNAL_DECODER_MEMORY_CAP / (1024 * 1024)
                    ),
                )
                .param("tool", kind.label()),
            ));
        }
        Err(super::process::CappedError::Engine(EngineError::DependencyMissing(f)))
            if f.reason.is_none() =>
        {
            return Err(EngineError::DependencyMissing(Failure::new(
                crate::engine::deps::Dependency::HeifDecoder.id(),
                f.detail,
            )));
        }
        Err(super::process::CappedError::Engine(other)) => return Err(other),
    }
    let Some(decoded) = decoded_png(&png) else {
        let detail = super::process::log_tail(&log, 400);
        return Err(EngineError::ConversionFailed(
            Failure::new(
                "heif_decode_failed",
                if detail.is_empty() {
                    format!("{} could not read this HEIC/HEIF file", kind.label())
                } else {
                    format!(
                        "{} could not read this HEIC/HEIF file: {detail}",
                        kind.label()
                    )
                },
            )
            .param("tool", kind.label()),
        ));
    };
    progress(0.5);
    super::image::to_pdf(&decoded, output, progress)
}

/// How far Windows Imaging Component got before it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WicStage {
    /// Creating the imaging factory or finding a decoder for the file.
    Open,
    /// A decoder accepted the file; reading its frame or pixels failed.
    Decode,
}

/// `WINCODEC_ERR_COMPONENTNOTFOUND`: no decoder claims the file — for a
/// `.heic` that means the Store codec is not installed.
pub const WINCODEC_COMPONENT_NOT_FOUND: i32 = 0x8898_2F50_u32 as i32;
/// `WINCODEC_ERR_UNKNOWNIMAGEFORMAT`, the other "no codec" answer.
pub const WINCODEC_UNKNOWN_IMAGE_FORMAT: i32 = 0x8898_2F07_u32 as i32;
/// `WINCODEC_ERR_COMPONENTINITIALIZEFAILURE`: Windows 11 registers a HEIF
/// decoder stub that fails to initialise when the Store codec behind it is
/// not installed (observed on a clean Windows 11 VM).
pub const WINCODEC_COMPONENT_INIT_FAILURE: i32 = 0x8898_2F8B_u32 as i32;

/// Turn a WIC failure into the error the queue acts on. Pure.
///
/// - No decoder for the file, or a decoder stub that cannot initialise → the
///   HEIF codec is missing (FR-058), whatever the stage.
/// - The decoder was found but decoding failed, and `hevc_available` says
///   there is no HEVC video decoder → *HEVC Video Extensions* is missing: the
///   HEIF extension parses the container and hands the pixels to HEVC, so
///   this is the shape of "HEIF installed, HEVC not". It pauses the job with
///   the remediation instead of burning retries on a generic failure.
/// - Anything else is a real decode failure (a damaged file).
///
/// `hevc_available` is only asked in the one case where it matters.
pub fn classify_wic_failure(
    stage: WicStage,
    code: i32,
    detail: &str,
    hevc_available: impl FnOnce() -> bool,
) -> EngineError {
    if matches!(
        code,
        WINCODEC_COMPONENT_NOT_FOUND
            | WINCODEC_UNKNOWN_IMAGE_FORMAT
            | WINCODEC_COMPONENT_INIT_FAILURE
    ) {
        return EngineError::DependencyMissing(Failure::new("heif-codec", WINDOWS_CODEC_MISSING));
    }
    if stage == WicStage::Decode && !hevc_available() {
        return EngineError::DependencyMissing(Failure::new("hevc-codec", WINDOWS_HEVC_MISSING));
    }
    EngineError::ConversionFailed(Failure::new(
        "heif_decode_failed",
        format!("could not decode the HEIF image: {detail}"),
    ))
}

/// Run `work` on its own thread and wait for it, honouring `cancel` and
/// `timeout` while it runs.
///
/// An in-process decoder cannot be killed the way a converter process is, so
/// on cancel or timeout the thread is **abandoned**: it finishes (or not) on
/// its own and its result is dropped, while the job moves on at once. Cancel
/// is [`EngineError::Cancelled`]; a timeout is the same terminal "did not
/// finish within" [`EngineError::ConversionTimedOut`] the process-based
/// converters report (`process::timed_out_error`).
pub fn run_abandonable<T: Send + 'static>(
    label: &str,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    work: impl FnOnce() -> Result<T, EngineError> + Send + 'static,
) -> Result<T, EngineError> {
    const POLL: Duration = Duration::from_millis(50);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("{label}-decode"))
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| {
            EngineError::ConversionFailed(
                Failure::new(
                    "converter_start_failed",
                    format!("could not start {label}: {e}"),
                )
                .param("tool", label),
            )
        })?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(EngineError::Cancelled);
        }
        if let Some(limit) = timeout {
            if started.elapsed() >= limit {
                return Err(super::process::timed_out_error(label, Some(limit)));
            }
        }
        match rx.recv_timeout(POLL) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(EngineError::ConversionFailed(
                    Failure::new("converter_crashed", format!("{label} stopped unexpectedly"))
                        .param("tool", label),
                ))
            }
        }
    }
}

/// Convert with the `sips` at `sips` (macOS), writing the PDF to `output`.
pub fn convert_with_sips(
    sips: &Path,
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), EngineError> {
    let png = work.path().join("heif.png");
    let mut cmd = std::process::Command::new(sips);
    cmd.args(sips_args(input, &png)).current_dir(work.path());
    let log = work.path().join("sips.log");
    super::process::run_bounded(cmd, "sips", cancel, timeout, &log)?;
    if !png.is_file() {
        return Err(EngineError::ConversionFailed(Failure::new(
            "heif_decode_failed",
            "the system image converter could not read this HEIC/HEIF file",
        )));
    }
    super::image::to_pdf(&png, output, progress)
}

/// Convert `input` to a PDF at `output` with this OS's decoder. `decoder` is
/// the external decoder the engine resolved on Linux
/// ([`crate::engine::deps::Dependency::HeifDecoder`]); macOS and Windows
/// ignore it.
pub fn convert(
    decoder: Option<&Path>,
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), EngineError> {
    #[cfg(target_os = "macos")]
    {
        let _ = decoder;
        let sips = Path::new("/usr/bin/sips");
        if !sips.is_file() {
            return Err(EngineError::DependencyMissing(Failure::new(
                "sips",
                "sips, the macOS image tool, could not be found",
            )));
        }
        convert_with_sips(sips, input, output, work, cancel, timeout, progress)
    }
    #[cfg(windows)]
    {
        let _ = (decoder, work);
        progress(0.0);
        // WIC decodes in-process and cannot be interrupted, so it runs on a
        // thread the job can walk away from on cancel or timeout.
        let source = input.to_path_buf();
        let (width, height, rgb) = run_abandonable(WIC_LABEL, cancel, timeout, move || {
            wic::decode_rgb8(&source)
        })?;
        progress(0.5);
        super::image::rgb_to_pdf(width, height, rgb, output)?;
        progress(1.0);
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        use crate::engine::deps::{self, Dependency};
        let Some(decoder) = decoder else {
            let spec = deps::spec(Dependency::HeifDecoder, deps::Os::current());
            return Err(EngineError::DependencyMissing(Failure::new(
                Dependency::HeifDecoder.id(),
                spec.remediation(),
            )));
        };
        convert_with_external(decoder, input, output, work, cancel, timeout, progress)
    }
}

/// Whether an HEVC decoder is installed (the FR-058 dependency report asks).
#[cfg(windows)]
pub use wic::hevc_decoder_available;

/// Windows Imaging Component decoding.
#[cfg(windows)]
mod wic {
    use std::path::Path;

    use windows::core::{GUID, HSTRING};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };

    use super::{classify_wic_failure, WicStage};
    use crate::engine::{EngineError, Failure};

    /// Same pixel ceiling as the built-in decoder (`image::decode_rgb8`).
    const MAX_SIDE: u32 = 30_000;

    /// Balances a successful `CoInitializeEx` on this (blocking-pool) thread.
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: paired with the successful CoInitializeEx below on
                // the same thread.
                unsafe { CoUninitialize() };
            }
        }
    }

    fn failed(stage: WicStage) -> impl Fn(windows::core::Error) -> EngineError {
        move |e| classify_wic_failure(stage, e.code().0, &e.to_string(), hevc_decoder_available)
    }

    /// Whether Media Foundation has any HEVC video decoder — software,
    /// hardware, or the Store's *HEVC Video Extensions* (a Store MFT, so
    /// `MFT_ENUM_FLAG_UNTRUSTED_STOREMFT` must be asked for explicitly). If
    /// the question itself fails the answer is "yes", so a damaged file is
    /// never misreported as a missing extension.
    pub fn hevc_decoder_available() -> bool {
        use windows::Win32::Media::MediaFoundation::{
            IMFActivate, MFMediaType_Video, MFTEnumEx, MFVideoFormat_HEVC,
            MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ALL, MFT_REGISTER_TYPE_INFO,
        };
        use windows::Win32::System::Com::CoTaskMemFree;
        /// `MFT_ENUM_FLAG_UNTRUSTED_STOREMFT` (mfapi.h).
        const UNTRUSTED_STOREMFT: i32 = 0x400;

        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_HEVC,
        };
        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: valid in/out pointers; the returned array is released below.
        let enumerated = unsafe {
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG(MFT_ENUM_FLAG_ALL.0 | UNTRUSTED_STOREMFT),
                Some(&input),
                None,
                &mut activates,
                &mut count,
            )
        };
        if !activates.is_null() {
            for n in 0..count as usize {
                // SAFETY: `count` initialised entries; dropping releases them.
                unsafe { std::ptr::drop_in_place(activates.add(n)) };
            }
            // SAFETY: the array was allocated by MFTEnumEx with CoTaskMemAlloc.
            unsafe { CoTaskMemFree(Some(activates.cast())) };
        }
        enumerated.is_err() || count > 0
    }

    pub fn decode_rgb8(input: &Path) -> Result<(u32, u32, Vec<u8>), EngineError> {
        // SAFETY: plain COM calls on interfaces owned by this function; every
        // out-pointer is a live local and the pixel buffer is sized from the
        // reported dimensions before CopyPixels writes into it.
        unsafe {
            // S_OK / S_FALSE both need a matching CoUninitialize; a thread
            // already in another apartment (RPC_E_CHANGED_MODE) still lets us
            // create in-proc objects, but must not be uninitialised by us.
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _guard = ComGuard(hr.is_ok());

            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(failed(WicStage::Open))?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    &HSTRING::from(input.as_os_str()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(failed(WicStage::Open))?;
            // From here a decoder has accepted the file: a failure is either
            // a missing HEVC decoder or a damaged image.
            let decode = failed(WicStage::Decode);
            let frame = decoder.GetFrame(0).map_err(&decode)?;
            let converter = factory.CreateFormatConverter().map_err(&decode)?;
            let rgb24: GUID = GUID_WICPixelFormat24bppRGB;
            converter
                .Initialize(
                    &frame,
                    &rgb24,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(&decode)?;
            let (mut width, mut height) = (0u32, 0u32);
            converter
                .GetSize(&mut width, &mut height)
                .map_err(&decode)?;
            if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
                return Err(EngineError::ConversionFailed(
                    Failure::new(
                        "heif_size_unsupported",
                        format!("unsupported HEIF image size {width}×{height}"),
                    )
                    .param("width", width)
                    .param("height", height),
                ));
            }
            let stride = width * 3;
            let mut pixels = vec![0u8; stride as usize * height as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .map_err(&decode)?;
            Ok((width, height, pixels))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_052_sips_args_convert_to_png_without_a_shell() {
        assert_eq!(
            sips_args(Path::new("/p/IMG 1.HEIC"), Path::new("/w/heif.png")),
            [
                "-s",
                "format",
                "png",
                "/p/IMG 1.HEIC",
                "--out",
                "/w/heif.png"
            ]
        );
    }

    /// FR-052 on Linux: each external decoder gets its own argument vector,
    /// chosen by name, never a shell string.
    #[test]
    fn fr_052_external_decoder_args_by_program_name() {
        let input = Path::new("/p/IMG 1.HEIC");
        let png = Path::new("/w/heif.png");
        for name in ["heif-dec", "heif-convert"] {
            let kind = ExternalDecoder::of(&Path::new("/usr/bin").join(name)).unwrap();
            assert_eq!(kind, ExternalDecoder::Libheif);
            assert_eq!(kind.args(input, png), ["/p/IMG 1.HEIC", "/w/heif.png"]);
        }
        for name in ["magick", "convert"] {
            let kind = ExternalDecoder::of(&Path::new("/usr/bin").join(name)).unwrap();
            assert_eq!(kind, ExternalDecoder::ImageMagick);
            assert_eq!(
                kind.args(input, png),
                ["heic:/p/IMG 1.HEIC[0]", "png:/w/heif.png"]
            );
        }
        assert_eq!(ExternalDecoder::of(Path::new("/usr/bin/sips")), None);
        assert_eq!(ExternalDecoder::Libheif.label(), "libheif");
    }

    /// libheif numbers its output when a file holds several images; the
    /// first one is the photo.
    #[test]
    fn fr_052_a_numbered_libheif_output_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("heif.png");
        assert_eq!(decoded_png(&png), None);
        std::fs::write(dir.path().join("heif-2.png"), b"x").unwrap();
        assert_eq!(decoded_png(&png), None);
        std::fs::write(dir.path().join("heif-1.png"), b"x").unwrap();
        assert_eq!(decoded_png(&png), Some(dir.path().join("heif-1.png")));
        std::fs::write(&png, b"x").unwrap();
        assert_eq!(decoded_png(&png), Some(png));
    }

    #[test]
    fn fr_058_windows_codec_message_names_the_store_extension() {
        assert!(WINDOWS_CODEC_MISSING.contains("HEIF Image Extensions"));
        assert!(WINDOWS_CODEC_MISSING.contains("Microsoft Store"));
    }

    /// FR-058: with the HEIF extension but no HEVC decoder, WIC finds a
    /// decoder and then fails on the pixels. That pauses with the message
    /// naming *HEVC Video Extensions*, not a retried generic failure.
    #[test]
    fn fr_058_a_missing_hevc_decoder_is_a_dependency_not_a_failure() {
        let missing_hevc = classify_wic_failure(WicStage::Decode, -2147024809, "x", || false);
        assert!(
            matches!(&missing_hevc, EngineError::DependencyMissing(m)
                if *m == WINDOWS_HEVC_MISSING && m.reason == Some("hevc-codec")),
            "{missing_hevc:?}"
        );
        assert!(WINDOWS_HEVC_MISSING.contains("HEVC Video Extensions"));
        // No decoder at all, or Windows 11's decoder stub without the Store
        // codec behind it: the HEIF codec message, at any stage.
        for stage in [WicStage::Open, WicStage::Decode] {
            for code in [
                WINCODEC_COMPONENT_NOT_FOUND,
                WINCODEC_UNKNOWN_IMAGE_FORMAT,
                WINCODEC_COMPONENT_INIT_FAILURE,
            ] {
                let e = classify_wic_failure(stage, code, "x", || true);
                assert!(matches!(&e, EngineError::DependencyMissing(m)
                    if *m == WINDOWS_CODEC_MISSING && m.reason == Some("heif-codec")));
            }
        }
        // HEVC present: a damaged file stays a conversion failure.
        let damaged = classify_wic_failure(WicStage::Decode, -1, "bad data", || true);
        assert!(matches!(&damaged, EngineError::ConversionFailed(m)
            if m.contains("bad data") && m.reason == Some("heif_decode_failed")));
        // HEVC is not asked about when the decoder was never found.
        let open = classify_wic_failure(WicStage::Open, -1, "denied", || panic!("not asked"));
        assert!(matches!(open, EngineError::ConversionFailed(_)));
    }

    /// FR-030/NFR-012: an in-process decode that hangs must not hold the job:
    /// cancel and the conversion timeout both end the wait at once.
    #[test]
    fn fr_058_a_hung_decode_is_abandoned_on_cancel_and_timeout() {
        let hang = || -> Result<(), EngineError> {
            std::thread::sleep(Duration::from_secs(30));
            Ok(())
        };
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let timed_out = run_abandonable(WIC_LABEL, &cancel, Some(Duration::from_millis(200)), hang);
        // Terminal like every converter timeout (not retried), and named.
        assert!(
            matches!(&timed_out, Err(EngineError::ConversionTimedOut(m))
                if m.contains("did not finish")
                    && m.reason == Some("tool_timed_out")
                    && m.params["tool"] == WIC_LABEL),
            "{timed_out:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        let cancel = AtomicBool::new(true);
        let cancelled = run_abandonable("decoder", &cancel, None, hang);
        assert!(matches!(cancelled, Err(EngineError::Cancelled)));

        let ok = run_abandonable("decoder", &AtomicBool::new(false), None, || Ok(7));
        assert_eq!(ok.unwrap(), 7);
    }

    /// The pages of the PDF at `pdf`.
    #[cfg(unix)]
    fn pages_of(pdf: &Path) -> usize {
        lopdf::Document::load(pdf).unwrap().get_pages().len()
    }

    /// A fake external decoder named `name` in `dir` that leaves `<dir>/ran`
    /// behind when it is started, records `MAGICK_CONFIGURE_PATH` and the
    /// policy it names into `<dir>/env`, and "decodes" by copying a real PNG
    /// to its last argument (without ImageMagick's `png:` prefix).
    ///
    /// It is returned once it can be started: on Linux a test thread that
    /// forks while the script is still open for writing holds it open in the
    /// child until that child execs, and starting the script then fails with
    /// `ETXTBSY`. A probe run (`FAKE_PROBE`, which exits before leaving any
    /// trace) is retried until it starts.
    #[cfg(unix)]
    fn fake_decoder(dir: &Path, name: &str) -> PathBuf {
        let png = dir.join("real.png");
        ::image::RgbImage::from_pixel(4, 3, ::image::Rgb([10, 200, 10]))
            .save_with_format(&png, ::image::ImageFormat::Png)
            .unwrap();
        let path = dir.join(name);
        let body = format!(
            r#"#!/bin/sh
[ -n "$FAKE_PROBE" ] && exit 0
: > '{ran}'
printf '%s\n' "${{MAGICK_CONFIGURE_PATH:-unset}}" "${{MAGICK_TEMPORARY_PATH:-unset}}" > '{env}'
[ -n "$MAGICK_CONFIGURE_PATH" ] && cat "$MAGICK_CONFIGURE_PATH/policy.xml" >> '{env}'
for last in "$@"; do :; done
cp '{png}' "${{last#png:}}"
"#,
            ran = dir.join("ran").display(),
            env = dir.join("env").display(),
            png = png.display(),
        );
        write_fake(&path, &body)
    }

    /// Write the `/bin/sh` script `body` (which must exit at once, leaving
    /// nothing behind, when `FAKE_PROBE` is set) to `path` as an executable,
    /// and return `path` once it can be started (see [`fake_decoder`]).
    #[cfg(unix)]
    fn write_fake(path: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..100 {
            match std::process::Command::new(path)
                .env("FAKE_PROBE", "1")
                .status()
            {
                Ok(status) if status.success() => return path.to_path_buf(),
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                other => panic!("{} does not start: {other:?}", path.display()),
            }
        }
        panic!("{} stayed busy", path.display());
    }

    /// A `.heic` that is really something else, or no HEIF at all.
    #[cfg(unix)]
    fn disguised_inputs() -> Vec<(&'static str, Vec<u8>)> {
        let mut huge = b"\xff\xff\xff\xf0ftypheic\0\0\0\0mif1heic".to_vec();
        huge.resize(64, 0);
        vec![
            (
                "svg",
                br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///etc/passwd"/></svg>"#
                    .to_vec(),
            ),
            (
                "mvg",
                b"push graphic-context\nviewbox 0 0 640 480\nimage over 0,0 0,0 'ephemeral:/etc/passwd'\npop graphic-context\n"
                    .to_vec(),
            ),
            (
                "postscript",
                b"%!PS-Adobe-3.0\n(/etc/passwd) (r) file\nshowpage\n".to_vec(),
            ),
            ("pbm", b"P1\n4 4\n0 1 0 1\n1 0 1 0\n0 1 0 1\n1 0 1 0\n".to_vec()),
            ("empty", Vec::new()),
            ("truncated ftyp", b"\0\0\0\x18ftypheic\0\0\0\0".to_vec()),
            ("huge ftyp", huge),
        ]
    }

    /// NFR-security, FR-052 on Linux: a `.heic` that is not HEIF is refused
    /// as a damaged photo before any decoder process starts, through each
    /// kind of decoder; a real HEIF header does reach the decoder (the fake
    /// would have recorded every refused start), and ImageMagick is started
    /// with the policy. The header checked is the private copy's.
    #[cfg(unix)]
    #[test]
    fn nfr_security_a_disguised_heic_never_reaches_a_decoder() {
        for name in ["heif-dec", "heif-convert", "magick", "convert"] {
            let dir = tempfile::tempdir().unwrap();
            let decoder = fake_decoder(dir.path(), name);
            for (what, bytes) in disguised_inputs() {
                let input = dir.path().join("IMG_0001.HEIC");
                std::fs::write(&input, &bytes).unwrap();
                let output = dir.path().join("o.pdf");
                let work = WorkDir::new("disguised").unwrap();
                let err = convert_with_external(
                    &decoder,
                    &input,
                    &output,
                    &work,
                    &AtomicBool::new(false),
                    None,
                    &|_| {},
                )
                .unwrap_err();
                assert!(
                    matches!(&err, EngineError::ConversionFailed(f)
                        if f.reason == Some("heif_decode_failed")
                            && f.contains("not a HEIC/HEIF photo")
                            // The user's file is named, not the copy.
                            && f.contains("IMG_0001.HEIC")),
                    "{name}, {what}: {err:?}"
                );
                assert!(
                    !dir.path().join("ran").exists(),
                    "{name} was started for a disguised {what}"
                );
                assert!(!output.exists(), "{name}, {what}");
                // The staged copy was removed and nothing was prepared for a
                // decoder.
                assert_eq!(
                    std::fs::read_dir(work.path()).unwrap().count(),
                    0,
                    "{name}, {what}"
                );
            }

            // The control: a HEIF header is let through to the same decoder.
            let input = dir.path().join("IMG_0002.HEIC");
            std::fs::write(&input, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
            let output = dir.path().join("o.pdf");
            let work = WorkDir::new("heif").unwrap();
            convert_with_external(
                &decoder,
                &input,
                &output,
                &work,
                &AtomicBool::new(false),
                None,
                &|_| {},
            )
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(dir.path().join("ran").exists(), "{name}");
            assert_eq!(pages_of(&output), 1, "{name}");
            let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
            let mut lines = env.lines();
            let (config, temp) = (lines.next().unwrap(), lines.next().unwrap());
            if ExternalDecoder::of(&decoder) == Some(ExternalDecoder::ImageMagick) {
                assert_eq!(
                    Path::new(config),
                    work.path().join("magick-policy"),
                    "{name}"
                );
                assert_eq!(Path::new(temp), work.path().join("magick-tmp"), "{name}");
                assert!(env.ends_with(MAGICK_POLICY), "{name}: {env}");
            } else {
                assert_eq!((config, temp), ("unset", "unset"), "{name}");
            }
        }
    }

    /// NFR-security (TOCTOU): the header check and the decoder read the same
    /// private copy. A fake decoder that swaps the user's file for an SVG the
    /// moment it starts still finds the HEIF bytes that were checked, at the
    /// staged path, and never the original's path in its argv.
    #[cfg(unix)]
    #[test]
    fn nfr_security_the_decoder_reads_the_copy_the_header_check_read() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("IMG_0003.HEIC");
        let header = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
        std::fs::write(&input, header).unwrap();
        let png = dir.path().join("real.png");
        ::image::RgbImage::from_pixel(2, 2, ::image::Rgb([1, 2, 3]))
            .save_with_format(&png, ::image::ImageFormat::Png)
            .unwrap();
        let decoder = write_fake(
            &dir.path().join("heif-convert"),
            &format!(
                r#"#!/bin/sh
[ -n "$FAKE_PROBE" ] && exit 0
printf '<svg/>' > '{input}'
printf '%s\n' "$1" > '{argv}'
cp "$1" '{seen}'
cp '{png}' "$2"
"#,
                input = input.display(),
                argv = dir.path().join("argv").display(),
                seen = dir.path().join("seen").display(),
                png = png.display(),
            ),
        );
        let work = WorkDir::new("toctou").unwrap();
        let output = dir.path().join("o.pdf");
        convert_with_external(
            &decoder,
            &input,
            &output,
            &work,
            &AtomicBool::new(false),
            None,
            &|_| {},
        )
        .unwrap();
        let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
        assert_eq!(Path::new(argv.trim_end()), work.path().join(STAGED_NAME));
        assert_eq!(std::fs::read(dir.path().join("seen")).unwrap(), header);
        assert_eq!(std::fs::read(&input).unwrap(), b"<svg/>");
        assert_eq!(pages_of(&output), 1);
    }

    /// NFR-security: a FIFO (or a device) named `.heic` is refused at once,
    /// before anything is copied or started: opening a FIFO with no writer,
    /// or reading one, would block the conversion for good.
    #[cfg(unix)]
    #[test]
    fn nfr_security_a_fifo_named_heic_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let decoder = fake_decoder(dir.path(), "heif-convert");
        let fifo = dir.path().join("IMG_0004.HEIC");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        for input in [fifo, PathBuf::from("/dev/zero")] {
            let work = WorkDir::new("fifo").unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let (shown, decoder) = (input.clone(), decoder.clone());
            let out = dir.path().join("o.pdf");
            // On its own thread, so a conversion that blocks fails the test
            // instead of hanging it.
            std::thread::spawn(move || {
                let r = convert_with_external(
                    &decoder,
                    &input,
                    &out,
                    &work,
                    &AtomicBool::new(false),
                    None,
                    &|_| {},
                );
                let _ = tx.send(r);
            });
            let err = rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| panic!("{} blocked the conversion", shown.display()))
                .unwrap_err();
            assert!(
                matches!(&err, EngineError::ConversionFailed(f)
                    if f.reason == Some("cannot_read_input")
                        && f.contains("not a regular file")),
                "{}: {err:?}",
                shown.display()
            );
        }
        assert!(!dir.path().join("ran").exists(), "a decoder was started");
    }

    /// The policy closes every domain before opening the one use, in the
    /// order ImageMagick needs (the last match wins).
    #[test]
    fn nfr_security_the_imagemagick_policy_allows_only_heif_in_png_out() {
        let at = |needle: &str| {
            MAGICK_POLICY
                .find(needle)
                .unwrap_or_else(|| panic!("policy lacks {needle}"))
        };
        for closed in [
            r#"<policy domain="delegate" rights="none" pattern="*"/>"#,
            r#"<policy domain="filter" rights="none" pattern="*"/>"#,
            r#"<policy domain="path" rights="none" pattern="@*"/>"#,
        ] {
            at(closed);
        }
        let coders = at(r#"<policy domain="coder" rights="none" pattern="*"/>"#);
        assert!(coders < at(r#"<policy domain="coder" rights="read" pattern="{HEIC,HEIF}"/>"#));
        assert!(coders < at(r#"<policy domain="coder" rights="write" pattern="PNG"/>"#));
        let modules = at(r#"<policy domain="module" rights="none" pattern="*"/>"#);
        assert!(
            modules < at(r#"<policy domain="module" rights="read | write" pattern="{HEIC,PNG}"/>"#)
        );
        // Nothing else is opened.
        assert_eq!(MAGICK_POLICY.matches(r#"rights="read"#).count(), 2);
        assert_eq!(MAGICK_POLICY.matches(r#"rights="write"#).count(), 1);
        for limit in [
            "memory",
            "map",
            "disk",
            "width",
            "height",
            "area",
            "list-length",
        ] {
            at(&format!(r#"name="{limit}""#));
        }
    }

    /// A HEIC made by the local ImageMagick (which encodes through libheif),
    /// or `None` with a note.
    #[cfg(target_os = "linux")]
    fn real_heic(dir: &Path) -> Option<PathBuf> {
        let png = dir.join("photo.png");
        let mut img = ::image::RgbImage::new(64, 48);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = ::image::Rgb([(x * 4) as u8, (y * 5) as u8, 90]);
        }
        img.save_with_format(&png, ::image::ImageFormat::Png)
            .unwrap();
        let heic = dir.join("photo.HEIC");
        let made = ["heif-enc", "magick", "convert"].iter().any(|tool| {
            let mut cmd = std::process::Command::new(tool);
            if *tool == "heif-enc" {
                cmd.arg("-o").arg(&heic).arg(&png);
            } else {
                cmd.arg(&png).arg(format!("heic:{}", heic.display()));
            }
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
                && heic.is_file()
        });
        if !made {
            eprintln!("SKIPPED heic: nothing here can encode HEIC");
            return None;
        }
        Some(heic)
    }

    /// An installed program named `name` on `PATH`.
    #[cfg(target_os = "linux")]
    fn on_path(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    }

    /// FR-052 on Linux with the real decoders: a real HEIC passes the header
    /// check and prints as one page through each installed decoder,
    /// ImageMagick under its policy; a disguised one is refused.
    #[cfg(target_os = "linux")]
    #[test]
    fn fr_052_a_real_heic_passes_the_gate_and_each_real_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let Some(heic) = real_heic(dir.path()) else {
            return;
        };
        header::check(&heic).expect("a real HEIC has a HEIF header");
        let mut ran = 0;
        for name in crate::engine::deps::HEIF_DECODERS_LINUX {
            let Some(decoder) = on_path(name) else {
                eprintln!("SKIPPED {name}: not installed");
                continue;
            };
            let output = dir.path().join(format!("{name}.pdf"));
            let work = WorkDir::new(name).unwrap();
            convert_with_external(
                &decoder,
                &heic,
                &output,
                &work,
                &AtomicBool::new(false),
                Some(Duration::from_secs(120)),
                &|_| {},
            )
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(pages_of(&output), 1, "{name}");
            ran += 1;
        }
        eprintln!("{ran} real HEIF decoder(s) exercised through the gate");
    }

    /// NFR-security on Linux with the real ImageMagick: the environment
    /// [`imagemagick_policy_env`] returns makes it load the policy (it lists
    /// the file), decode a real HEIC to PNG, and refuse every other coder,
    /// whether named explicitly or picked from the content.
    #[cfg(target_os = "linux")]
    #[test]
    fn nfr_security_real_imagemagick_applies_the_policy() {
        let Some(magick) = on_path("magick").or_else(|| on_path("convert")) else {
            eprintln!("SKIPPED ImageMagick policy: not installed");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(heic) = real_heic(dir.path()) else {
            return;
        };
        let work = WorkDir::new("policy").unwrap();
        let env = imagemagick_policy_env(&work).unwrap();
        let run = |args: &[String]| {
            std::process::Command::new(&magick)
                .args(args)
                .envs(env.clone())
                .current_dir(work.path())
                .output()
                .unwrap()
        };
        let listed = run(&["-list".into(), "policy".into()]);
        let listed = String::from_utf8_lossy(&listed.stdout);
        let policy = work.path().join("magick-policy").join("policy.xml");
        assert!(
            listed.contains(&policy.display().to_string()),
            "policy not loaded:\n{listed}"
        );

        let png = work.path().join("heif.png");
        let ok = run(&[
            format!("heic:{}[0]", heic.display()),
            format!("png:{}", png.display()),
        ]);
        assert!(ok.status.success() && png.is_file(), "{ok:?}");

        let svg = work.path().join("x.svg");
        std::fs::write(
            &svg,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#,
        )
        .unwrap();
        let ps = work.path().join("x.ps");
        std::fs::write(&ps, b"%!PS-Adobe-3.0\nshowpage\n").unwrap();
        let out = |n: &str| format!("png:{}", work.path().join(n).display());
        for (what, args) in [
            (
                "svg by content",
                vec![svg.display().to_string(), out("a.png")],
            ),
            (
                "svg by name",
                vec![format!("svg:{}", svg.display()), out("b.png")],
            ),
            (
                "postscript",
                vec![format!("ps:{}", ps.display()), out("c.png")],
            ),
            (
                "png read",
                vec![format!("png:{}", png.display()), out("d.png")],
            ),
            (
                "gif write",
                vec![
                    format!("heic:{}[0]", heic.display()),
                    format!("gif:{}", work.path().join("e.gif").display()),
                ],
            ),
            (
                "@file",
                vec![format!("heic:@{}", svg.display()), out("f.png")],
            ),
        ] {
            let refused = run(&args);
            assert!(!refused.status.success(), "{what} was allowed: {refused:?}");
        }
        for n in ["a.png", "b.png", "c.png", "d.png", "e.gif", "f.png"] {
            assert!(!work.path().join(n).exists(), "{n} was written");
        }
        // The refusals are the policy's, not a missing delegate's.
        let gif = run(&[
            format!("heic:{}[0]", heic.display()),
            format!("gif:{}", work.path().join("e.gif").display()),
        ]);
        assert!(
            String::from_utf8_lossy(&gif.stderr).contains("security policy"),
            "{gif:?}"
        );
    }

    /// FR-058 on a real Windows: a `.heic` WIC cannot decode pauses with one
    /// of the two remediation messages whenever a codec is missing, and the
    /// HEVC probe answers without failing.
    #[cfg(windows)]
    #[test]
    fn fr_058_windows_heic_without_codecs_names_the_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let heic = dir.path().join("IMG_0001.HEIC");
        std::fs::write(&heic, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
        let hevc = wic::hevc_decoder_available();
        eprintln!("HEVC decoder available: {hevc}");
        let cancel = AtomicBool::new(false);
        let work = WorkDir::new("heif-test").unwrap();
        let err = convert(
            None,
            &heic,
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            Some(Duration::from_secs(60)),
            &|_| {},
        )
        .expect_err("a header-only file cannot decode");
        eprintln!("{err:?}");
        match err {
            EngineError::DependencyMissing(m) => {
                assert!(
                    m == WINDOWS_CODEC_MISSING || m == WINDOWS_HEVC_MISSING,
                    "{m}"
                )
            }
            EngineError::ConversionFailed(_) => assert!(hevc, "no HEVC decoder must pause"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
