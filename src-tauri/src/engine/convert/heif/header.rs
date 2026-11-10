//! The ISO-BMFF header check every `.heic`/`.heif` passes before an external
//! decoder sees it (Linux).
//!
//! The extension is the user's word, not the file's: a `.heic` can hold an
//! SVG, a PostScript program or an ImageMagick MVG script, and a decoder that
//! picks its reader from the content (ImageMagick does, and its readers start
//! delegate programs) would act on it. So the file must first look like what
//! the app claims to print: an ISO base media file whose first box is a
//! well-formed `ftyp` naming a HEIF brand. Anything else is refused as a
//! damaged photo (`heif_decode_failed`) without starting a process.
//!
//! Only a bounded prefix is read ([`PREFIX_LEN`]), so the check costs the
//! same for a 4 KB file and a 4 GB one.
//!
//! **Brands accepted** ([`HEIF_BRANDS`]) are the ones ISO/IEC 23008-12 (HEIF)
//! defines for still images and image sequences: the structural brands
//! `mif1`, `mif2` and `msf1`, and the HEVC-coded ones (`heic`, `heix`,
//! `heim`, `heis` for images, `hevc`, `hevx`, `hevm`, `hevs` for sequences).
//! AVIF (`avif`, `avis`) is not on the list: the app supports `.heic` and
//! `.heif`, not `.avif`. An AVIF file is nevertheless a HEIF file and says so
//! with `mif1`/`msf1` among its compatible brands, so one renamed `.heif`
//! passes like any other HEIF and goes to the same libheif decoder; what the
//! check keeps out is content that is not HEIF at all, not a codec inside it.

use std::io::Read;
use std::path::Path;

use crate::engine::{EngineError, Failure};

/// The HEIF brands an `ftyp` must name, as its major brand or among its
/// compatible brands (see the module documentation).
pub const HEIF_BRANDS: &[[u8; 4]] = &[
    *b"mif1", *b"mif2", *b"msf1", *b"heic", *b"heix", *b"heim", *b"heis", *b"hevc", *b"hevx",
    *b"hevm", *b"hevs",
];

/// How many bytes of the file are read: an `ftyp` larger than this is not one
/// any encoder writes (Apple's lists six brands in 40 bytes).
pub const PREFIX_LEN: usize = 4096;

/// The smallest `ftyp`: size, type, major brand and minor version.
const FTYP_MIN: usize = 16;

/// Why a prefix is not a HEIF header; the text goes into the failure detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotHeif {
    /// Shorter than the smallest `ftyp` (an empty file included).
    TooShort,
    /// The first box is not `ftyp`.
    NoFtyp,
    /// The `ftyp` size is below the minimum, not a whole number of brands,
    /// or a 0 ("to end of file") or 1 (64-bit size) escape.
    BadSize,
    /// The `ftyp` claims more bytes than the file (or [`PREFIX_LEN`]) holds.
    Truncated,
    /// A well-formed `ftyp` that names no HEIF brand.
    NoHeifBrand,
}

impl NotHeif {
    fn describe(self) -> &'static str {
        match self {
            Self::TooShort => "it is too short to be a HEIF file",
            Self::NoFtyp => "it does not start with an ISO-BMFF ftyp box",
            Self::BadSize => "its ftyp box has an invalid size",
            Self::Truncated => "its ftyp box is truncated",
            Self::NoHeifBrand => "its ftyp box names no HEIF brand",
        }
    }
}

/// Whether `prefix`, the first bytes of a file (at most [`PREFIX_LEN`]),
/// starts with an `ftyp` box naming a HEIF brand. Pure.
pub fn check_prefix(prefix: &[u8]) -> Result<(), NotHeif> {
    if prefix.len() < FTYP_MIN {
        return Err(NotHeif::TooShort);
    }
    if &prefix[4..8] != b"ftyp" {
        return Err(NotHeif::NoFtyp);
    }
    let size = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    // 0 and 1 are escapes (box runs to end of file; 64-bit size follows):
    // legal for a media box, never written for an ftyp, and both fall below
    // the minimum. The brands after the minor version are four bytes each.
    if size < FTYP_MIN || !(size - FTYP_MIN).is_multiple_of(4) {
        return Err(NotHeif::BadSize);
    }
    if size > prefix.len() {
        return Err(NotHeif::Truncated);
    }
    let major = [prefix[8], prefix[9], prefix[10], prefix[11]];
    let (compatible, _) = prefix[FTYP_MIN..size].as_chunks::<4>();
    if HEIF_BRANDS.contains(&major) || compatible.iter().any(|b| HEIF_BRANDS.contains(b)) {
        Ok(())
    } else {
        Err(NotHeif::NoHeifBrand)
    }
}

/// The `cannot_read_input` failure for `input`, which could not be opened or
/// read (`e`).
pub(crate) fn unreadable(input: &Path, e: &std::io::Error) -> EngineError {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    EngineError::ConversionFailed(
        Failure::new(
            "cannot_read_input",
            format!("cannot read {}: {e}", input.display()),
        )
        .param("file", name),
    )
}

/// Open `path` for reading, refusing anything but a regular file.
///
/// A `.heic` can be a FIFO or a device: opening a FIFO with no writer, or
/// reading one, blocks for as long as nobody writes, and a device such as
/// `/dev/zero` never ends. So the file is opened with `O_NONBLOCK` (which a
/// regular file ignores), and the handle itself, not the path, is asked what
/// it is: a path swapped after the question cannot change the answer.
pub(crate) fn open_regular(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

/// Refuse `input` unless its header is a HEIF one ([`check_prefix`]),
/// reading at most [`PREFIX_LEN`] bytes. A file that cannot be opened or read,
/// or is not a regular file ([`open_regular`]), is `cannot_read_input`; one
/// that is not HEIF is `heif_decode_failed`.
pub fn check(input: &Path) -> Result<(), EngineError> {
    check_as(input, input)
}

/// [`check`] on `file`, the private copy of the user's `shown`, which the
/// failure names instead (the copy's own name means nothing to the user).
pub fn check_as(file: &Path, shown: &Path) -> Result<(), EngineError> {
    let file = open_regular(file).map_err(|e| unreadable(shown, &e))?;
    let mut prefix = Vec::with_capacity(PREFIX_LEN);
    file.take(PREFIX_LEN as u64)
        .read_to_end(&mut prefix)
        .map_err(|e| unreadable(shown, &e))?;
    check_prefix(&prefix).map_err(|why| {
        EngineError::ConversionFailed(Failure::new(
            "heif_decode_failed",
            format!(
                "{} is not a HEIC/HEIF photo: {}",
                shown.display(),
                why.describe()
            ),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `ftyp` box with `major` and `compatible` brands.
    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = (FTYP_MIN + 4 * compatible.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(major);
        b.extend_from_slice(&[0, 0, 0, 0]);
        for c in compatible {
            b.extend_from_slice(*c);
        }
        b
    }

    /// The headers real encoders write pass: Apple's (`heic` + `MiHE`…),
    /// libheif's, and a HEIF whose only HEIF brand is a compatible one.
    #[test]
    fn fr_052_real_heif_headers_pass() {
        let mut apple = ftyp(
            b"heic",
            &[b"mif1", b"MiHE", b"MiPr", b"miaf", b"MiHB", b"heic"],
        );
        apple.extend_from_slice(b"\0\0\0\x0bmeta");
        assert_eq!(check_prefix(&apple), Ok(()));
        assert_eq!(check_prefix(&ftyp(b"mif1", &[b"heic", b"mif1"])), Ok(()));
        assert_eq!(check_prefix(&ftyp(b"msf1", &[])), Ok(()));
        assert_eq!(check_prefix(&ftyp(b"miaf", &[b"hevc"])), Ok(()));
        for brand in HEIF_BRANDS {
            assert_eq!(check_prefix(&ftyp(brand, &[])), Ok(()), "{brand:?}");
        }
    }

    /// AVIF is not a supported format on its own; one that declares itself
    /// HEIF (`mif1`, as AVIF encoders do) is a HEIF file.
    #[test]
    fn fr_052_avif_passes_only_as_a_declared_heif() {
        assert_eq!(
            check_prefix(&ftyp(b"avif", &[b"avif", b"miaf"])),
            Err(NotHeif::NoHeifBrand)
        );
        assert_eq!(
            check_prefix(&ftyp(b"avif", &[b"avif", b"mif1", b"miaf"])),
            Ok(())
        );
    }

    /// NFR-security: what is not a HEIF header is refused, whatever it is.
    #[test]
    fn nfr_security_non_heif_headers_are_refused() {
        let cases: &[(&[u8], NotHeif)] = &[
            (b"", NotHeif::TooShort),
            (b"\0\0\0\x18ftypheic", NotHeif::TooShort),
            (
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                NotHeif::NoFtyp,
            ),
            (b"push graphic-context\nviewbox 0 0 1 1\n", NotHeif::NoFtyp),
            (b"%!PS-Adobe-3.0\n(x) show showpage\n", NotHeif::NoFtyp),
            (b"P1\n1 1\n0\n                ", NotHeif::NoFtyp),
            // A video, not a photo: an ISO-BMFF file of another family.
            (&ftyp(b"isom", &[b"iso2", b"mp41"]), NotHeif::NoHeifBrand),
            (&ftyp(b"avif", &[]), NotHeif::NoHeifBrand),
        ];
        for (bytes, why) in cases {
            assert_eq!(check_prefix(bytes), Err(*why), "{bytes:?}");
        }
    }

    /// The `ftyp` size must be sane and inside the bytes read.
    #[test]
    fn nfr_security_an_ftyp_with_a_bad_size_is_refused() {
        let mut b = ftyp(b"heic", &[b"mif1"]);
        let with_size = |b: &mut Vec<u8>, size: u32| b[..4].copy_from_slice(&size.to_be_bytes());
        for (size, why) in [
            (0, NotHeif::BadSize),
            (1, NotHeif::BadSize),
            (15, NotHeif::BadSize),
            (22, NotHeif::BadSize),
            (24, NotHeif::Truncated),
            (u32::MAX - 3, NotHeif::Truncated),
        ] {
            with_size(&mut b, size);
            assert_eq!(check_prefix(&b), Err(why), "size {size}");
        }
        with_size(&mut b, 20);
        assert_eq!(check_prefix(&b), Ok(()));
    }

    /// The file check reads a bounded prefix and maps each outcome to its
    /// reason: a big file with a HEIF header passes after 4 KB, a missing
    /// file is unreadable, anything else a damaged photo.
    #[test]
    fn nfr_security_the_file_check_reads_a_bounded_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.heic");
        let mut bytes = ftyp(b"heic", &[b"mif1"]);
        bytes.resize(8 * 1024 * 1024, 0xAB);
        std::fs::write(&big, &bytes).unwrap();
        check(&big).unwrap();

        let svg = dir.path().join("photo.heic");
        std::fs::write(&svg, b"<svg/>").unwrap();
        let err = check(&svg).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f)
                if f.reason == Some("heif_decode_failed") && f.contains("not a HEIC/HEIF")),
            "{err:?}"
        );

        #[cfg(unix)]
        {
            // A FIFO and a device are refused at once, never read.
            let fifo = dir.path().join("fifo.heic");
            let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            // SAFETY: a valid NUL-terminated path.
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            for special in [fifo.as_path(), Path::new("/dev/zero")] {
                let err = check(special).unwrap_err();
                assert!(
                    matches!(&err, EngineError::ConversionFailed(f)
                        if f.reason == Some("cannot_read_input")
                            && f.contains("not a regular file")),
                    "{err:?}"
                );
            }
        }

        let gone = dir.path().join("gone.heic");
        let err = check(&gone).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f)
                if f.reason == Some("cannot_read_input") && f.params["file"] == "gone.heic"),
            "{err:?}"
        );
    }
}
