//! Masking for the bridge's log records (NFR-082).
//!
//! The documents PluryFact sends are tax records: their names and labels can
//! carry a taxpayer number (RUC, 13 digits), an ID number (cédula, 10) or a
//! 49-digit access key (clave de acceso). The bridge logs counts, sizes and
//! codes, not names; whatever text does reach a record goes through
//! [`mask_identifiers`] first.

/// Runs of at least this many digits are treated as an identifier.
pub const MIN_IDENTIFIER_DIGITS: usize = 10;

/// Replace every run of [`MIN_IDENTIFIER_DIGITS`] or more ASCII digits with
/// `*` except its last three digits, so a record can still tell two
/// documents apart without naming the taxpayer: `0999999999001` becomes
/// `**********001`.
pub fn mask_identifiers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= MIN_IDENTIFIER_DIGITS {
            let keep = 3;
            out.extend(std::iter::repeat_n('*', run.len() - keep));
            out.push_str(&run[run.len() - keep..]);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// An origin as it may appear in a log: an extension id is public
/// information, but the argument is untrusted, so it is bounded and stripped
/// of control characters.
pub fn origin_for_log(origin: &str) -> String {
    origin
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_082_identifiers_are_masked() {
        assert_eq!(mask_identifiers("0999999999001"), "**********001");
        assert_eq!(
            mask_identifiers("Recibidos · 1790011674001 · octubre 2026"),
            "Recibidos · **********001 · octubre 2026"
        );
        let clave = "1".repeat(49);
        let masked = mask_identifiers(&format!("RIDE_{clave}.pdf"));
        assert_eq!(masked, format!("RIDE_{}111.pdf", "*".repeat(46)));
        // Cédula (10 digits) too.
        assert_eq!(mask_identifiers("0912345678"), "*******678");
        // Short numbers (sequentials, years) are left alone.
        assert_eq!(
            mask_identifiers("factura 001-001-000000123 2026"),
            "factura 001-001-000000123 2026"
        );
    }

    #[test]
    fn nfr_082_origins_are_bounded_and_printable() {
        assert_eq!(
            origin_for_log("chrome-extension://abc/\n\u{1b}[31m"),
            "chrome-extension://abc/[31m"
        );
        assert_eq!(origin_for_log(&"x".repeat(500)).len(), 100);
    }
}
