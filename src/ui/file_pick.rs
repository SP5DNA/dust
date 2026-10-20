//! A button that chooses a file and reads it as text: a word list, a
//! backup to restore.
//!
//! On the web and the desktop it is a file input under a label dressed as
//! a button. The app builds for phones cannot use one: there the webview
//! renderer asks its native side for the file a file input chose, and on
//! Android and iOS the native side gives none back, so a file input on a
//! phone never yields a file. So on a phone the button opens the webview's
//! own picker instead, through a file input made outside the app's page,
//! where the renderer does not reach, and the file is read there and handed
//! back.

use dioxus::html::FileData;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::ui::widgets::{Icon, IconName};

/// Whether this build chooses files through the webview's own picker.
const WEBVIEW_PICKER: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// A file chosen, as the button hands it over: its name and size, and its
/// text, unless it was larger than the button reads or could not be read as
/// text.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChosenFile {
    pub name: String,
    pub size: u64,
    pub text: Option<String>,
}

/// Read a file a file input chose, up to `max_bytes`.
async fn read_chosen(file: FileData, max_bytes: u64) -> ChosenFile {
    let (name, size) = (file.name(), file.size());
    let text = if size > max_bytes {
        None
    } else {
        file.read_string().await.ok()
    };
    ChosenFile { name, size, text }
}

/// The script that has the element `id` open the webview's own file picker
/// for `accept`, and sends back each file chosen as a [`ChosenFile`]. The
/// file input it opens is the document body's own, outside the page the
/// renderer draws, so the renderer leaves its clicks alone. A picker made
/// before for the same id, by a button drawn before, gives way to this one.
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
fn picker_script(id: &str, accept: &str, max_bytes: u64) -> String {
    format!(
        r##"const id = {id:?};
const pickers = window.__dustPickers || (window.__dustPickers = {{}});
if (pickers[id]) {{
    pickers[id]();
}}
const input = document.createElement("input");
input.type = "file";
input.accept = {accept:?};
input.style.display = "none";
document.body.appendChild(input);
const open = (event) => {{
    if (!(event.target instanceof Element) || !event.target.closest("#" + id)) {{
        return;
    }}
    event.preventDefault();
    event.stopPropagation();
    input.value = "";
    input.click();
}};
document.addEventListener("click", open, true);
input.addEventListener("change", () => {{
    const file = input.files && input.files[0];
    if (!file) {{
        return;
    }}
    const chosen = (text) => dioxus.send({{ name: file.name, size: file.size, text }});
    if (file.size > {max_bytes}) {{
        chosen(null);
        return;
    }}
    const reader = new FileReader();
    reader.onload = () => chosen(String(reader.result));
    reader.onerror = () => chosen(null);
    reader.readAsText(file);
}});
pickers[id] = () => {{
    document.removeEventListener("click", open, true);
    input.remove();
}};"##
    )
}

/// A button that chooses a file of the kinds `accept` names, reads it as
/// text up to `max_bytes`, and hands it to `on_choose`.
#[component]
pub fn FileButton(
    /// The button's id, and its file input's.
    id: String,
    /// The kinds of file offered, as an `accept` attribute.
    accept: &'static str,
    max_bytes: u64,
    icon: IconName,
    label: &'static str,
    on_choose: EventHandler<ChosenFile>,
) -> Element {
    // Moved on by every file chosen, to draw a fresh file input: the same
    // file chosen again in the old one would change nothing, and not load.
    let mut picked = use_signal(|| 0u64);
    if WEBVIEW_PICKER {
        let script = picker_script(&id, accept, max_bytes);
        use_hook(move || {
            spawn(async move {
                let mut picker = document::eval(&script);
                while let Ok(file) = picker.recv::<ChosenFile>().await {
                    on_choose.call(file);
                }
            })
        });
        return rsx! {
            button { id, class: "btn btn-secondary btn-sm", r#type: "button",
                Icon { name: icon }
                "{label}"
            }
        };
    }
    rsx! {
        label { class: "btn btn-secondary btn-sm file-button",
            for fresh in [picked()] {
                input {
                    key: "{fresh}",
                    id: id.clone(),
                    class: "file-input",
                    r#type: "file",
                    accept,
                    onchange: move |e| {
                        if let Some(file) = e.files().into_iter().next() {
                            picked += 1;
                            spawn(async move {
                                on_choose.call(read_chosen(file, max_bytes).await);
                            });
                        }
                    },
                }
            }
            Icon { name: icon }
            "{label}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::picker_script;

    /// The picker is the body's own, opened from the button with the click
    /// that pressed it, and a file too large is handed back unread.
    #[test]
    fn the_phone_picker_opens_from_its_button_and_reads_the_file() {
        let script = picker_script("file-backup", ".json", 500);
        assert!(script.contains(r#"const id = "file-backup";"#), "{script}");
        assert!(script.contains(r#"input.accept = ".json";"#), "{script}");
        assert!(script.contains("document.body.appendChild(input);"));
        assert!(script.contains(r##"event.target.closest("#" + id)"##));
        assert!(script.contains("input.click();"));
        assert!(script.contains("if (file.size > 500)"));
        assert!(script.contains("reader.readAsText(file);"));
        assert!(script.contains("dioxus.send("));
        // Drawn again, it takes over from the picker before.
        let takeover = script.find("pickers[id]();").expect("takes over");
        let made = script.find("document.createElement").expect("makes one");
        assert!(takeover < made, "{script}");
    }
}
