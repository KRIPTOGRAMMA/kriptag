// A guard over the two-accent palette (v0.9.94).
//
// Both properties checked here are invisible at a glance and silently
// reversible. The gradient rules in `.btn-primary` have existed since long
// before the second accent had a value of its own: when `--accent-secondary`
// equals `--accent`, `linear-gradient` composes a colour with itself and the
// button renders as a flat fill — it looks deliberate, not broken. The same goes
// for the neutrals: nobody notices #f5f5f5 creeping back in place of #f4f2f8.
//
// The check lives on the Rust side for the reason spelled out in
// comments.test.ts: Vite runs stylesheets through its own pipeline, so
// `import.meta.glob` returns an EMPTY string for .css under every form of ?raw.
// A vitest guard would have listed the path and scanned nothing. `include_str!`
// reads the actual file, the same bridge i18n.rs and mock_guard.rs already use.
//
// What this does NOT check: that the two accents look good together, or that any
// component actually uses the second one. Those are taste and wiring; this is
// only the invariant that the palette has two distinct accents and no pure grey.

#[cfg(test)]
mod tests {
    const CSS: &str = include_str!("../../src/app.css");

    // The token block of one theme: `:root { … }` for light, `.dark { … }` for
    // dark. Slicing to the closing brace keeps a later block's tokens from
    // leaking into the search.
    fn theme_block(selector: &str) -> &'static str {
        let head = format!("{selector} {{");
        let at = CSS
            .find(&head)
            .unwrap_or_else(|| panic!("в app.css не найден блок {selector}"));
        let rest = &CSS[at..];
        let end = rest
            .find("\n}")
            .unwrap_or_else(|| panic!("блок {selector} не закрыт"));
        &rest[..end]
    }

    fn token(block: &str, name: &str) -> String {
        let needle = format!("--{name}:");
        let at = block
            .find(&needle)
            .unwrap_or_else(|| panic!("токен --{name} не объявлен"));
        let rest = &block[at + needle.len()..];
        let end = rest
            .find(';')
            .unwrap_or_else(|| panic!("токен --{name} без точки с запятой"));
        rest[..end].trim().to_string()
    }

    #[test]
    fn second_accent_differs_from_the_first_in_both_themes() {
        for selector in [":root", ".dark"] {
            let block = theme_block(selector);
            let accent = token(block, "accent");
            let secondary = token(block, "accent-secondary");
            assert_ne!(
                accent, secondary,
                "{selector}: --accent-secondary равен --accent, \
                 градиент .btn-primary вырождается в плоскую заливку"
            );
        }
    }

    // The frontend keeps its own copy of these defaults: an `<input type="color">`
    // cannot render "unset", so the swatch of an untouched field has to show the
    // value the theme would give it. That copy cannot be derived at runtime —
    // getComputedStyle returns the *current* value, including the user's override,
    // which is exactly what the placeholder must not show. So it is duplicated,
    // and this test is what keeps the duplicate honest.
    #[test]
    fn color_defaults_ts_matches_app_css() {
        const TS: &str = include_str!("../../src/lib/colorDefaults.ts");

        // The two record literals, in the order they appear in the module.
        fn ts_block(after: &str) -> &'static str {
            let at = TS
                .find(after)
                .unwrap_or_else(|| panic!("в colorDefaults.ts не найден {after}"));
            let rest = &TS[at..];
            let end = rest.find("\n};").expect("литерал не закрыт");
            &rest[..end]
        }

        fn ts_value(block: &str, key: &str) -> String {
            let needle = format!("{key}:");
            let at = block
                .find(&needle)
                .unwrap_or_else(|| panic!("в colorDefaults.ts нет ключа {key}"));
            let rest = &block[at + needle.len()..];
            let end = rest.find(',').expect("значение без запятой");
            rest[..end].trim().trim_matches('"').to_string()
        }

        // color_* on the frontend, --* in the stylesheet.
        let pairs = [
            ("color_accent", "accent"),
            ("color_accent_secondary", "accent-secondary"),
            ("color_bg", "bg-primary"),
            ("color_bg_secondary", "bg-secondary"),
            ("color_bg_hover", "bg-hover"),
            ("color_bg_card", "bg-card"),
            ("color_text_secondary", "text-secondary"),
            ("color_text", "text-primary"),
            ("color_border", "border"),
        ];

        for (selector, ts_marker) in [(":root", "const LIGHT"), (".dark", "const DARK")] {
            let css_block = theme_block(selector);
            let ts = ts_block(ts_marker);
            for (ts_key, css_token) in pairs {
                assert_eq!(
                    ts_value(ts, ts_key),
                    token(css_block, css_token),
                    "{ts_marker}.{ts_key} разошёлся с app.css {selector} --{css_token}"
                );
            }
        }
    }

    // The engine draws the popup of a <select>, scrollbars and the date-picker
    // calendar itself, and it decides light or dark from color-scheme alone —
    // tokens have no say. Both themes must declare it, or those parts stay light
    // on a dark background.
    #[test]
    fn both_themes_declare_a_color_scheme() {
        for (selector, expected) in [(":root", "light"), (".dark", "dark")] {
            let block = theme_block(selector);
            let at = block
                .find("color-scheme:")
                .unwrap_or_else(|| panic!("{selector}: color-scheme не объявлен"));
            let rest = &block[at + "color-scheme:".len()..];
            let end = rest.find(';').expect("color-scheme без точки с запятой");
            assert_eq!(rest[..end].trim(), expected, "{selector}: не та схема");
        }
    }

    #[test]
    fn neutral_surfaces_are_not_pure_grey() {
        // A pure grey has R == G == B. The violet lean is what separates this
        // palette from the default grey it grew out of, and it is one careless
        // revert away from being lost.
        fn is_pure_grey(hex: &str) -> bool {
            let h = hex.trim_start_matches('#');
            h.len() == 6 && h.is_char_boundary(2) && h[0..2] == h[2..4] && h[2..4] == h[4..6]
        }

        for selector in [":root", ".dark"] {
            let block = theme_block(selector);
            for name in ["bg-secondary", "bg-hover", "border"] {
                let value = token(block, name);
                assert!(
                    !is_pure_grey(&value),
                    "{selector}: --{name} = {value} — чистый серый"
                );
            }
        }
    }

    /// WCAG relative luminance. Lives here rather than being shared with
    /// surfaces.ts because this guard runs where app.css is readable — vitest
    /// gets an empty string for any .css import, see the note at the top.
    fn luminance(hex: &str) -> f64 {
        let h = hex.trim_start_matches('#');
        let channel = |i: usize| {
            let v = u8::from_str_radix(&h[i..i + 2], 16).expect("не hex") as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn solid_category_chip_keeps_its_label_readable() {
        // .chip-cat--solid fills the chip with the category's colour and puts the
        // label on top, its colour picked per chip by onAccentText().
        //
        // The bar is AA for body text. It has to be 4.5: the worst possible fill
        // is a mid-grey (#7a7a7a), and even there the better of black and white
        // reaches 4.29 — so any looser threshold is one no colour can fail, and
        // the guard would be decoration.
        //
        // NB: the chips on screen read their colour from the categories table,
        // not from these tokens (see migration 0015). What this pins down is the
        // shipped palette. An existing user's colours cannot be fixed from here,
        // and the migration must not be edited to chase them — sqlx verifies the
        // checksums of migrations it has already applied.
        for selector in [":root", ".dark"] {
            let block = theme_block(selector);
            for name in ["cat-work", "cat-study", "cat-home", "cat-health", "cat-other"] {
                let fill = token(block, name);
                let best = contrast("#ffffff", &fill).max(contrast("#141414", &fill));
                assert!(
                    best >= 4.5,
                    "{selector}: --{name} = {fill} — подпись на сплошном чипе {best:.2}, \
                     ни белая, ни тёмная не читается"
                );
            }
        }
    }
}
