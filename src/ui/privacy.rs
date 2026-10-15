//! The store listing and the in-app link point at the same page.
//! `docs/privacy.html` is copied onto GitHub Pages; this card is what a
//! reviewer finds without leaving Settings, including while offline.

/// Public page Play Console should list. Served as a normal HTML document,
/// not a download.
pub const PRIVACY_POLICY_URL: &str = "https://mashu.github.io/dust/privacy/";

use dioxus::prelude::*;

#[component]
pub fn PrivacyCard() -> Element {
    rsx! {
        div { class: "card stack-sm legal",
            div { class: "card-head",
                div { class: "card-head-main",
                    div {
                        h3 { class: "card-title", "Privacy" }
                        p { class: "card-note", "Practice stays on this device" }
                    }
                }
            }
            p { class: "muted",
                "Dust stores your settings and practice history only on this device: speed and alphabet, the groups that were sent, what you typed, and the scores. There is no account, and that history is not uploaded. The newest 500 sessions are kept, and any of them can be deleted under Stats → History."
            }
            p { class: "muted",
                "The app does load its typefaces from Google Fonts. That request includes your IP address. Your practice is not part of it. Google is the only third party the app contacts."
            }
            p { class: "muted",
                "Dust does not use the microphone, location, or contacts, and it has no ads or analytics. Uninstalling the app deletes what it stored."
            }
            p { class: "muted",
                a {
                    class: "privacy-link",
                    href: PRIVACY_POLICY_URL,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "Privacy policy"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_hosted_policy_is_about_this_app() {
        let policy = include_str!("../../docs/privacy.html");
        assert!(policy.contains("dev.dust.morse"));
        assert!(policy.contains("fonts.googleapis.com"));
        assert!(policy.contains("fonts.gstatic.com"));
        let flat = policy.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("does not record the microphone"));
        assert!(flat.contains("does not sell personal information"));
        assert!(policy.contains("https://github.com/mashu/dust/issues"));
        assert!(
            !policy.to_ascii_lowercase().contains(".pdf"),
            "Play rejects a policy that is a downloadable file"
        );
    }
}
