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

use std::path::Path;
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
    /// of the file (`[0]`) and is told the output format explicitly (`png:`),
    /// whatever the name ends in. Neither is asked to rotate: libheif already
    /// applies the HEIF orientation while decoding, and rotating again by the
    /// EXIF tag would turn the photo twice.
    pub fn args(self, input: &Path, png: &Path) -> Vec<String> {
        let input = input.to_string_lossy().into_owned();
        let png = png.to_string_lossy().into_owned();
        match self {
            Self::Libheif => vec![input, png],
            Self::ImageMagick => vec![format!("{input}[0]"), format!("png:{png}")],
        }
    }
}

/// The PNG an external decoder wrote for `png`. libheif writes one file per
/// top-level image and numbers them (`heif-1.png`, `heif-2.png`…) when the
/// file holds more than one; the first is the photo. `None` when it wrote
/// nothing.
fn decoded_png(png: &Path) -> Option<std::path::PathBuf> {
    if png.is_file() {
        return Some(png.to_path_buf());
    }
    let stem = png.file_stem()?.to_str()?;
    let first = png.with_file_name(format!("{stem}-1.png"));
    first.is_file().then_some(first)
}

/// Convert with the external decoder at `decoder` (Linux), writing the PDF
/// to `output`.
///
/// A decoder that disappeared between being found and being started pauses
/// the job like a missing one (`heif-decoder`); one that runs and writes no
/// image could not read the file (`heif_decode_failed`).
pub fn convert_with_external(
    decoder: &Path,
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), EngineError> {
    // Anything that is not a known decoder is treated as libheif's
    // interface, the simplest one.
    let kind = ExternalDecoder::of(decoder).unwrap_or(ExternalDecoder::Libheif);
    let png = work.path().join("heif.png");
    let mut cmd = std::process::Command::new(decoder);
    cmd.args(kind.args(input, &png)).current_dir(work.path());
    let log = work.path().join("heif-decoder.log");
    progress(0.0);
    super::process::run_bounded(cmd, kind.label(), cancel, timeout, &log).map_err(|e| match e {
        EngineError::DependencyMissing(f) if f.reason.is_none() => EngineError::DependencyMissing(
            Failure::new(crate::engine::deps::Dependency::HeifDecoder.id(), f.detail),
        ),
        other => other,
    })?;
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
                ["/p/IMG 1.HEIC[0]", "png:/w/heif.png"]
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
