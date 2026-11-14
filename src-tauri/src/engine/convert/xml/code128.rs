//! Code 128 barcodes as inline SVG (FR-057b): the access key (*clave de
//! acceso*) of an SRI receipt printed under its digits, as on every RIDE.
//!
//! Generated here, in Rust, with no dependency and nothing fetched: the page
//! the web view prints may not load anything (see [`super::super::html`]).
//!
//! Encoding is deliberately simple and deterministic:
//!
//! - a string of digits uses code set C (two digits per symbol); an odd
//!   count ends with a switch to code set B for the last digit. A 49-digit
//!   access key is therefore Start C, 24 pairs, Code B, one digit;
//! - anything else uses code set B (printable ASCII, `' '..='~'`);
//! - characters outside printable ASCII cannot be encoded ([`encode`] is
//!   `None`), so a caller prints the text alone.
//!
//! The pattern table is ISO/IEC 15417's; the tests check it structurally and
//! against reference encodings produced by an independent implementation.

/// Bar/space widths of every symbol value, `0..=105`, then the stop pattern
/// at index 106. Each symbol is three bars and three spaces adding up to 11
/// modules; the stop is four bars and three spaces adding up to 13.
const PATTERNS: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212",
    "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221",
    "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221",
    "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321",
    "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131",
    "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111",
    "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114",
    "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242",
    "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311",
    "113141", "114131", "311141", "411131", "211412", "211214", "211232", "2331112",
];

const CODE_B: u8 = 100;
const START_B: u8 = 104;
const START_C: u8 = 105;
const STOP: usize = 106;

/// Quiet zone on each side, in modules (the standard asks for ten).
pub const QUIET_ZONE: usize = 10;

/// The symbol values for `data`, start code first, without the check symbol
/// and the stop. `None` when `data` is empty or holds a character code set B
/// cannot carry. Pure.
pub fn symbols(data: &str) -> Option<Vec<u8>> {
    if data.is_empty() {
        return None;
    }
    let bytes = data.as_bytes();
    if bytes.iter().all(u8::is_ascii_digit) && bytes.len() >= 2 {
        let mut out = vec![START_C];
        let pairs = bytes.len() / 2;
        for pair in bytes[..pairs * 2].chunks(2) {
            out.push((pair[0] - b'0') * 10 + (pair[1] - b'0'));
        }
        if bytes.len() % 2 == 1 {
            out.push(CODE_B);
            out.push(bytes[bytes.len() - 1] - b' ');
        }
        return Some(out);
    }
    let mut out = vec![START_B];
    for &b in bytes {
        if !(b' '..=b'~').contains(&b) {
            return None;
        }
        out.push(b - b' ');
    }
    Some(out)
}

/// The modulo-103 check symbol of `values` (start code first). Pure.
pub fn checksum(values: &[u8]) -> u8 {
    let sum: usize = values
        .iter()
        .enumerate()
        .map(|(i, &v)| usize::from(v) * i.max(1))
        .sum();
    (sum % 103) as u8
}

/// The whole barcode as a module string (`'1'` bar, `'0'` space), without
/// quiet zones: start, data, check symbol, stop. `None` as for [`symbols`].
/// Pure.
pub fn modules(data: &str) -> Option<String> {
    let mut values = symbols(data)?;
    values.push(checksum(&values));
    let mut out = String::with_capacity(values.len() * 11 + 13);
    let patterns = values
        .iter()
        .map(|&v| PATTERNS[usize::from(v)])
        .chain(std::iter::once(PATTERNS[STOP]));
    for pattern in patterns {
        for (i, w) in pattern.bytes().enumerate() {
            let bit = if i % 2 == 0 { '1' } else { '0' };
            for _ in 0..(w - b'0') {
                out.push(bit);
            }
        }
    }
    Some(out)
}

/// An inline SVG of the barcode for `data`, `height_mm` tall and as wide as
/// its container (the bars keep their proportions horizontally because the
/// viewBox is in modules and `preserveAspectRatio` is `none` only
/// vertically: every module scales by the same factor). `None` as for
/// [`symbols`]. Pure.
pub fn svg(data: &str, height_mm: f32) -> Option<String> {
    let modules = modules(data)?;
    let width = modules.len() + 2 * QUIET_ZONE;
    let mut rects = String::new();
    let bytes = modules.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'1' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'1' {
                i += 1;
            }
            rects.push_str(&format!(
                "<rect x=\"{}\" y=\"0\" width=\"{}\" height=\"40\"/>",
                start + QUIET_ZONE,
                i - start
            ));
        } else {
            i += 1;
        }
    }
    Some(format!(
        "<svg class=\"barcode\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} 40\" \
         preserveAspectRatio=\"none\" shape-rendering=\"crispEdges\" role=\"img\" \
         aria-label=\"Code 128\" style=\"height:{height_mm}mm\"><g fill=\"#000\">{rects}</g></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a module string back to symbol values with the same table, so
    /// a pattern typo that still sums to 11 cannot hide.
    fn decode(modules: &str) -> Vec<usize> {
        let bytes = modules.as_bytes();
        let mut widths = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let start = i;
            while i < bytes.len() && bytes[i] == bytes[start] {
                i += 1;
            }
            widths.push((b'0' + (i - start) as u8) as char);
        }
        let mut out = Vec::new();
        let mut rest: &[char] = &widths;
        while rest.len() >= 6 {
            if rest.len() == 7 {
                let stop: String = rest.iter().collect();
                assert_eq!(stop, PATTERNS[STOP]);
                out.push(STOP);
                break;
            }
            let sym: String = rest[..6].iter().collect();
            let v = PATTERNS[..STOP]
                .iter()
                .position(|p| *p == sym)
                .unwrap_or_else(|| panic!("unknown pattern {sym}"));
            out.push(v);
            rest = &rest[6..];
        }
        out
    }

    #[test]
    fn fr_057b_code128_table_is_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for (v, p) in PATTERNS.iter().enumerate() {
            let sum: u32 = p.bytes().map(|b| u32::from(b - b'0')).sum();
            if v == STOP {
                assert_eq!((p.len(), sum), (7, 13), "stop");
            } else {
                assert_eq!((p.len(), sum), (6, 11), "value {v}");
            }
            assert!(p.bytes().all(|b| (b'1'..=b'4').contains(&b)), "{v}");
            assert!(seen.insert(*p), "duplicate pattern at {v}");
        }
        // The well-known fixed symbols.
        assert_eq!(PATTERNS[0], "212222");
        assert_eq!(PATTERNS[103], "211412");
        assert_eq!(PATTERNS[104], "211214");
        assert_eq!(PATTERNS[105], "211232");
    }

    /// Reference encodings from python-barcode 0.15 (`Code128(s).build()`),
    /// an independent implementation, for each code set path.
    #[test]
    fn fr_057b_code128_matches_reference_vectors() {
        let cases = [
            (
                "123456",
                "11010011100101100111001000101100011100010110100011011101100011101011",
            ),
            (
                "PJJ123C",
                "1101001000011101110110101101110001011011100010011100110110011100101100101110010001000110111010001101100011101011",
            ),
            (
                "Pluryprint",
                "1101001000011101110110110010100001001111001010010011110110110111101010011110010010011110100001101001100001010010011110100101111001001100011101011",
            ),
            (
                "1411202601099999999900110010010000000011234567811",
                "110100111001001100111011000100100110010011101110010011011001101100110010010001011101111010111011110101110111101011101111011011001100110001001001101100110011001000100110011011001101100110011011001100110110011001101100110011000100100111011011101011101100010000101100100101111001011110111010011100110111101001001100011101011",
            ),
        ];
        for (data, expected) in cases {
            assert_eq!(modules(data).as_deref(), Some(expected), "{data}");
        }
    }

    #[test]
    fn fr_057b_code128_symbols_and_checksum() {
        // Set C: 105, 12, 34, 56; check (105 + 12 + 68 + 168) % 103 = 44.
        let v = symbols("123456").unwrap();
        assert_eq!(v, [105, 12, 34, 56]);
        assert_eq!(checksum(&v), 44);
        // An odd digit count ends in code set B.
        assert_eq!(symbols("12345").unwrap(), [105, 12, 34, 100, 21]);
        // A single digit is plain code set B.
        assert_eq!(symbols("7").unwrap(), [104, 23]);
        // Not encodable.
        assert_eq!(symbols(""), None);
        assert_eq!(symbols("año"), None);
        assert_eq!(symbols("a\nb"), None);
    }

    #[test]
    fn fr_057b_code128_round_trips_through_the_table() {
        let key = "1411202601099999999900110010010000000011234567811";
        let m = modules(key).unwrap();
        let mut values: Vec<usize> = symbols(key).unwrap().into_iter().map(usize::from).collect();
        values.push(usize::from(checksum(&symbols(key).unwrap())));
        values.push(STOP);
        assert_eq!(decode(&m), values);
        // Start, 24 pairs, Code B, one digit, check: 28 symbols of 11
        // modules, then the 13-module stop.
        assert_eq!(m.len(), 28 * 11 + 13);
    }

    #[test]
    fn fr_057b_code128_svg_is_self_contained() {
        let svg = svg("123456", 12.0).unwrap();
        assert!(svg.starts_with("<svg "), "{svg}");
        assert!(svg.contains("viewBox=\"0 0 88 40\""), "{svg}");
        assert!(!svg.contains("href"), "{svg}");
        assert!(!svg.contains("<script"), "{svg}");
        // First bar sits after the quiet zone, two modules wide.
        assert!(svg.contains("<rect x=\"10\" y=\"0\" width=\"2\""), "{svg}");
        assert_eq!(super::svg("ñ", 12.0), None);
    }
}
