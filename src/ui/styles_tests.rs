//! Layout invariants the screens depend on. The HTML tests never paint, so
//! these assertions catch the stylesheet drifting back to rules that shove
//! the nav off-center, hide sliders, or mix theme tokens.

#[test]
fn the_stylesheet_keeps_the_layout_the_ui_needs() {
    let css = include_str!("../../assets/styles.css");

    assert!(
        css.contains(".bottom-dock"),
        "the Practice/Stats/Settings bar has to be flex-centered, not left:50%"
    );
    assert!(
        !css.contains("transform: translateX(-50%)"),
        "centering with transform used to shove the bottom nav to the right"
    );
    assert!(
        !css.contains("\n  left: 50%;"),
        "left: 50% without a matching transform is the shifted-nav bug"
    );
    assert!(
        !css.contains("color-mix("),
        "color-mix leaves header/nav unthemed when the engine does not implement it"
    );
    assert!(
        !css.contains(":root:not([data-theme"),
        "dark tokens on :root:not([data-theme=light]) paint html dark while Auto is light"
    );
    assert!(
        !css.contains(".app-root:not([data-theme"),
        "Auto must not inherit dark tokens from :not([data-theme=light])"
    );
    assert!(
        css.contains(".app-root[data-theme=\"auto\"]"),
        "Auto is an explicit theme so the OS media query has a single target"
    );
    assert!(
        css.contains(".slider-bar") && css.contains(".slider-fill") && css.contains(".slider-knob"),
        "range thumbs are webkit/moz-only; the track has to be real boxes"
    );
    assert!(
        css.contains(".switch.on i { left: 22px"),
        "the switch knob has to move with left, not transform"
    );

    let modern_rgb = regex_rgb_slash(css);
    assert!(
        modern_rgb.is_empty(),
        "CSS Color 4 rgb() with slash alpha is not portable: {modern_rgb:?}"
    );
}

fn regex_rgb_slash(css: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let bytes = css.as_bytes();
    let needle = b"rgb(";
    let mut i = 0;
    while i + 4 < bytes.len() {
        if bytes[i..].starts_with(needle)
            && let Some(end) = css[i..].find(')')
        {
            let token = &css[i..i + end + 1];
            if token.contains('/') {
                hits.push(token.to_string());
            }
            i += end + 1;
            continue;
        }
        i += 1;
    }
    hits
}

const CSS: &str = include_str!("../../assets/styles.css");

/// The stylesheet with its comments taken out.
fn without_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// The declarations of the first rule whose selector is `selector`, one per
/// line, trimmed.
fn declarations(css: &str, selector: &str) -> Vec<String> {
    let start = css
        .find(&format!("{selector} {{"))
        .unwrap_or_else(|| panic!("no rule for {selector}"));
    let body = &css[start..];
    let open = body.find('{').expect("a rule opens") + 1;
    let close = open + body[open..].find('}').expect("a rule closes");
    body[open..close]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Auto in a dark system and pinned Dark are one palette written twice,
/// because one of them has to sit inside a media query. They must not drift.
#[test]
fn both_dark_palettes_are_the_same_palette() {
    let css = without_comments(CSS);
    let auto = declarations(&css, "  .app-root[data-theme=\"auto\"]");
    let dark = declarations(&css, "\n.app-root[data-theme=\"dark\"]");
    assert!(auto.len() > 30, "the auto palette was not found whole");
    assert_eq!(auto, dark);
}

/// Every string literal in the app's sources, split into the words a `class`
/// attribute is made of.
fn source_words() -> std::collections::BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).expect("the sources can be read") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push_str(&std::fs::read_to_string(&path).expect("a source file"));
            }
        }
    }
    let mut src = String::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut src,
    );
    let mut words = std::collections::BTreeSet::new();
    let mut chars = src.chars();
    let mut previous = ' ';
    while let Some(ch) = chars.next() {
        // A `'"'` character literal opens no string.
        if ch == '"' && previous != '\'' {
            let mut literal = String::new();
            while let Some(inner) = chars.next() {
                match inner {
                    '\\' => {
                        chars.next();
                    }
                    '"' => break,
                    _ => literal.push(inner),
                }
            }
            words.extend(literal.split_whitespace().map(str::to_string));
        }
        previous = ch;
    }
    words
}

/// Every class the stylesheet styles is drawn somewhere, and every token it
/// defines is read. A rule for markup that has gone is never noticed, which
/// is how ninety lines of them, and forty-odd tokens, built up.
#[test]
fn the_stylesheet_styles_only_what_the_app_draws() {
    let css = without_comments(CSS);
    // Selectors only: take the declaration blocks out, innermost first.
    let mut selectors = css.clone();
    while let Some(close) = selectors.find('}') {
        let open = selectors[..close].rfind('{').expect("braces pair up");
        selectors.replace_range(open..=close, " ");
    }
    let words = source_words();
    let mut unused: Vec<String> = selectors
        .match_indices('.')
        .map(|(at, _)| {
            selectors[at + 1..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
                .collect::<String>()
        })
        .filter(|name| name.starts_with(|ch: char| ch.is_ascii_alphabetic()))
        .filter(|name| !words.contains(name))
        .collect();
    unused.sort();
    unused.dedup();
    assert!(unused.is_empty(), "styled but never drawn: {unused:?}");

    let mut unread: Vec<String> = css
        .match_indices("--")
        .map(|(at, _)| {
            css[at..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                .collect::<String>()
        })
        .filter(|name| css.contains(&format!("{name}:")))
        .filter(|name| !css.contains(&format!("var({name})")))
        .collect();
    unread.sort();
    unread.dedup();
    assert!(unread.is_empty(), "defined but never read: {unread:?}");
}
