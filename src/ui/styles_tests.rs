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

/// Every rule that holds declarations, as its selector list and its
/// declarations split into property and value. At-rules such as `@media` are
/// looked through: only the rules inside them hold declarations.
fn rules(css: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut rules = Vec::new();
    let mut prelude = 0;
    let mut open: Option<(&str, usize)> = None;
    for (at, ch) in css.char_indices() {
        match ch {
            '{' => {
                open = Some((css[prelude..at].trim(), at + 1));
                prelude = at + 1;
            }
            '}' => {
                // A block that opened another, like `@media`, already gave
                // its place to the rule inside it.
                if let Some((selector, body)) = open.take() {
                    let declarations = css[body..at]
                        .split(';')
                        .filter_map(|declaration| declaration.split_once(':'))
                        .map(|(property, value)| (property.trim(), value.trim()))
                        .collect();
                    rules.push((selector, declarations));
                }
                prelude = at + 1;
            }
            _ => {}
        }
    }
    rules
}

/// The selectors of a selector list, split at the commas outside brackets,
/// so `:has(a, b)` stays whole.
fn selector_list(list: &str) -> Vec<&str> {
    let mut selectors = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (at, ch) in list.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                selectors.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    selectors.push(list[start..].trim());
    selectors
}

/// The element type a selector picks out, from its last compound: `body` for
/// `html > body:hover`, and nothing for `.app-root`.
fn subject_type(selector: &str) -> &str {
    let mut depth = 0;
    let mut start = 0;
    for (at, ch) in selector.char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' | '+' | '~' if depth == 0 => start = at + 1,
            _ if depth == 0 && ch.is_whitespace() => start = at + 1,
            _ => {}
        }
    }
    let compound = &selector[start..];
    let end = compound
        .find(|ch: char| !ch.is_ascii_alphanumeric())
        .unwrap_or(compound.len());
    &compound[..end]
}

/// What the block opened by `prelude` holds, nested rules and all.
fn block<'a>(css: &'a str, prelude: &str) -> &'a str {
    let start = css
        .find(&format!("{prelude} {{"))
        .unwrap_or_else(|| panic!("no block for {prelude}"));
    let open = start + css[start..].find('{').expect("a block opens") + 1;
    let mut depth = 1;
    for (at, ch) in css[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &css[open..open + at];
                }
            }
            _ => {}
        }
    }
    panic!("{prelude} never closes")
}

/// Only the root paints the paper, and only it is fixed to the window. On any
/// other box a fixed background puts WebKit on its slow scroll path, where
/// every scroll frame repaints the whole window and the desktop app waits on
/// each of them. The selector is plain `html` because the Linux window head
/// overrides this rule on source order alone (src/main.rs), which a more
/// specific rule, `:root` included, would quietly defeat.
#[test]
fn only_the_root_has_a_fixed_background() {
    let css = without_comments(CSS);
    let rules = rules(&css);
    let fixed: Vec<&str> = rules
        .iter()
        .filter(|(_, declarations)| {
            declarations.iter().any(|(property, value)| {
                property.starts_with("background")
                    && value
                        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
                        .any(|word| word == "fixed")
            })
        })
        .map(|(selector, _)| *selector)
        .collect();
    assert_eq!(fixed, ["html"], "only the root's background is fixed");

    let (_, paper) = rules
        .iter()
        .find(|(selector, declarations)| {
            *selector == "html"
                && declarations
                    .iter()
                    .any(|(property, _)| *property == "background-image")
        })
        .expect("the root paints the paper");
    let colours: Vec<&str> = paper
        .iter()
        .filter(|(property, _)| *property == "background-color")
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(
        colours,
        ["#f2e9d9", "var(--bg)"],
        "a literal colour comes first, for an engine without custom properties"
    );
}

/// Body stays transparent, so the root alone paints the paper. Body's box is
/// covered by .app-root anyway; a background of its own would only cost a
/// paint, and a fixed one a slow scroll on every frame.
#[test]
fn the_body_paints_no_background_of_its_own() {
    let css = without_comments(CSS);
    let body_rules: Vec<_> = rules(&css)
        .into_iter()
        .filter(|(selector, _)| {
            selector_list(selector)
                .iter()
                .any(|one| subject_type(one) == "body")
        })
        .collect();
    assert!(!body_rules.is_empty(), "the rules for body were not found");
    for (selector, declarations) in body_rules {
        let painted: Vec<_> = declarations
            .iter()
            .filter(|(property, _)| property.starts_with("background"))
            .collect();
        assert!(
            painted.is_empty(),
            "{selector} gives body a background of its own: {painted:?}"
        );
    }
}

/// The sending dot pulses for as long as a group is sent, and it grows a
/// box-shadow, which no engine composites: a smooth pulse repaints on every
/// frame. Held steps let the webview rest between them. The shorthand resets
/// the timing, so the steps have to follow it, and the shorthand's own easing
/// is what an engine without `jump-none` keeps.
#[test]
fn the_sending_dot_pulses_in_held_steps() {
    let css = without_comments(CSS);
    let live = declarations(&css, "\n.status-dot.live");
    let pulse = live
        .iter()
        .position(|line| line == "animation: pulse-dot 1.1s ease-in-out infinite;")
        .expect("the dot pulses for the whole send, smoothly where steps are unknown");
    let steps = live
        .iter()
        .position(|line| line == "animation-timing-function: steps(6, jump-none);")
        .expect("the pulse moves in held steps");
    assert!(
        pulse < steps,
        "the shorthand would undo the steps: {live:?}"
    );
}

/// Reduced motion stills every animation, the sending dot's among them, on
/// the pseudo-elements as well as the elements. `*` loses to every rule that
/// sets an animation, so only `!important` reaches them.
#[test]
fn reduced_motion_stills_every_animation() {
    let css = without_comments(CSS);
    let reduced = block(&css, "@media (prefers-reduced-motion: reduce)");
    let (selector, declarations) = rules(reduced)
        .into_iter()
        .find(|(selector, _)| selector_list(selector).contains(&"*"))
        .expect("reduced motion has a rule for every element");
    for every in ["*", "*::before", "*::after"] {
        assert!(
            selector_list(selector).contains(&every),
            "reduced motion leaves {every} moving"
        );
    }
    for property in ["animation-duration", "animation-iteration-count"] {
        assert!(
            declarations
                .iter()
                .any(|(name, value)| *name == property && value.ends_with("!important")),
            "reduced motion has to force {property}"
        );
    }
}
