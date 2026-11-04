//! What the pane sees and sends: frames of the active page while it watches, and the user's input.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use utoipa::ToSchema;

use super::driver::Driver;
use super::input::{KeyInput, MouseInput};

/// The narrowest and widest layouts a pane gives a page; a narrower pane shows it scaled down, so pages keep their
/// desktop layout.
const PANE_WIDTHS: (f64, f64) = (1280.0, 3840.0);
/// The tallest and widest shapes a pane may give a page, as height over width.
const PANE_SHAPES: (f64, f64) = (0.4, 2.5);

/// The viewport a browser's pane asks its pages for, shared by them; `None` until a pane has measured itself.
pub(crate) type PaneFit = std::sync::Arc<std::sync::Mutex<Option<(u64, u64)>>>;

/// One picture of the active page.
pub struct Frame {
    pub page: String,
    /// The page's size in CSS pixels, which pane coordinates map onto.
    pub width: f64,
    pub height: f64,
    pub jpeg: Vec<u8>,
}

#[derive(Serialize)]
struct Header<'a> {
    page: &'a str,
    width: f64,
    height: f64,
}

impl Frame {
    /// One binary message: a 4-byte little-endian header length, the JSON header, then the JPEG.
    pub fn encode(&self) -> Vec<u8> {
        let header = Header {
            page: &self.page,
            width: self.width,
            height: self.height,
        };
        let header = serde_json::to_vec(&header).expect("a frame header always serialises");
        let length = u32::try_from(header.len()).expect("a frame header is short");

        let mut message = Vec::with_capacity(4 + header.len() + self.jpeg.len());
        message.extend_from_slice(&length.to_le_bytes());
        message.extend_from_slice(&header);
        message.extend_from_slice(&self.jpeg);

        message
    }
}

/// What the pane sends on its view socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ViewInput {
    #[serde(rename = "mouse")]
    Mouse(MouseInput),
    #[serde(rename = "key")]
    Key(KeyInput),
    /// Pasted or composed text, typed into the focused element.
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "select")]
    Select { page: String },
    #[serde(rename = "close")]
    Close { page: String },
    #[serde(rename = "newPage")]
    NewPage {},
    /// An address to load in the active page, or `back`, `forward`, `reload`.
    #[serde(rename = "navigate")]
    Navigate { url: String },
    #[serde(rename = "dialog")]
    Dialog { accept: bool, text: Option<String> },
    /// The size in CSS pixels the pane shows the page at, so the page can be laid out to fill it.
    #[serde(rename = "fit")]
    Fit { width: f64, height: f64 },
}

/// What the engine tells a pane besides frames, as JSON text.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Notice {
    /// The CSS cursor the page shows under the pane's pointer.
    Cursor { cursor: String },
    /// Text this pane's click or shortcut copied in the browser, for the pane to put on its user's clipboard.
    Copied { text: String },
}

/// Applies the pane's input in the order it was sent. Moves and wheel turns that queued while the browser was busy
/// are folded together, so a browser that answers each one slowly (Firefox) never falls behind the pointer.
pub(crate) async fn apply_inputs(
    driver: Arc<dyn Driver>,
    mut queued: mpsc::UnboundedReceiver<ViewInput>,
    notices: mpsc::UnboundedSender<Notice>,
) {
    while let Some(first) = queued.recv().await {
        let mut batch = vec![first];
        while let Ok(next) = queued.try_recv() {
            batch.push(next);
        }

        for input in coalesce(batch) {
            let before = match may_copy(&input) {
                true => Some(driver.clipboard().await),
                false => None,
            };
            let _ = driver.input(input).await;
            if let Some(before) = before {
                tokio::spawn(follow_copy(driver.clone(), before, notices.clone()));
            }
        }
    }
}

/// A click, or a key that may run a copy button or a shortcut: Control or Meta held, Enter, or Space.
fn may_copy(input: &ViewInput) -> bool {
    match input {
        ViewInput::Mouse(mouse) => mouse.event == "up",
        ViewInput::Key(key) => key.event == "down" && (key.is_shortcut() || key.key == "Enter" || key.key == " "),
        _ => false,
    }
}

/// Looks at the clipboard a few times after the input, since a page often copies once a promise settles.
async fn follow_copy(driver: Arc<dyn Driver>, before: Clipboard, notices: mpsc::UnboundedSender<Notice>) {
    for wait in COPY_CHECKS {
        tokio::time::sleep(wait).await;
        if let Some(text) = driver.clipboard().await.copied_since(&before) {
            let _ = notices.send(Notice::Copied { text });
            return;
        }
    }
}

/// The waits between looks at the clipboard after an input that may copy: 0.1 s, 0.4 s and 1.2 s after it.
const COPY_CHECKS: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(800),
];

/// A headless browser keeps its own clipboard, so what its page copies is read back from the page and handed to the
/// pane. The page tells two things: its last `copy` or `cut` (by key, menu or `execCommand`, on any page), and on a
/// secure page the clipboard itself, which also catches `navigator.clipboard.writeText`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub(crate) struct Clipboard {
    #[serde(default)]
    pub(crate) copy: Option<Copied>,
    #[serde(default)]
    pub(crate) held: Option<String>,
}

/// One copy or cut, told apart from the last by its id.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct Copied {
    pub(crate) id: f64,
    pub(crate) text: String,
}

impl Clipboard {
    /// What an input copied: a new copy (what the clipboard now holds, or what was copied when the page cannot read
    /// it), else a change of the clipboard. Copying the same text again counts, as the user's clipboard may differ.
    fn copied_since(&self, before: &Self) -> Option<String> {
        let changed = self.held.clone().filter(|_| self.held != before.held);
        let copy = self.copy.as_ref().filter(|copy| before.copy.as_ref() != Some(*copy));
        let copied = copy.map(|copy| changed.clone().unwrap_or_else(|| copy.text.clone()));

        copied.or(changed).filter(|text| !text.is_empty())
    }
}

/// Folds each run of pointer moves and wheel turns into its last move and one turn summing the rest.
fn coalesce(batch: Vec<ViewInput>) -> Vec<ViewInput> {
    let mut folded = Vec::with_capacity(batch.len());
    let mut run = Run::default();

    for input in batch {
        match input {
            ViewInput::Mouse(mouse) if mouse.event == "move" => run.moved(mouse),
            ViewInput::Mouse(mouse) if mouse.event == "wheel" => run.turned(mouse, &mut folded),
            other => {
                run.flush(&mut folded);
                folded.push(other);
            }
        }
    }
    run.flush(&mut folded);

    folded
}

/// A run of moves and wheel turns waiting to be folded.
#[derive(Default)]
struct Run {
    last_move: Option<MouseInput>,
    turn: Option<MouseInput>,
    /// Whether the run ended on a move, which then goes last so the pointer stays where it was left.
    ends_moved: bool,
}

impl Run {
    fn moved(&mut self, mouse: MouseInput) {
        self.last_move = Some(mouse);
        self.ends_moved = true;
    }

    fn turned(&mut self, mouse: MouseInput, folded: &mut Vec<ViewInput>) {
        // A turn with other modifiers held (Ctrl to zoom) is a different gesture.
        if self.turn.as_ref().is_some_and(|turn| turn.modifiers != mouse.modifiers) {
            self.flush(folded);
        }
        self.turn = Some(match self.turn.take() {
            Some(sum) => MouseInput {
                delta_x: Some(sum.delta_x.unwrap_or_default() + mouse.delta_x.unwrap_or_default()),
                delta_y: Some(sum.delta_y.unwrap_or_default() + mouse.delta_y.unwrap_or_default()),
                ..mouse
            },
            None => mouse,
        });
        self.ends_moved = false;
    }

    fn flush(&mut self, folded: &mut Vec<ViewInput>) {
        let moved = self.last_move.take().map(ViewInput::Mouse);
        let turned = self.turn.take().map(ViewInput::Mouse);
        let (first, second) = if self.ends_moved {
            (turned, moved)
        } else {
            (moved, turned)
        };

        folded.extend(first);
        folded.extend(second);
    }
}

/// The viewport that fills a pane of this size: a desktop-wide layout in the pane's shape.
pub(crate) fn pane_viewport(width: f64, height: f64) -> Option<(u64, u64)> {
    if !(width.is_finite() && height.is_finite() && width >= 1.0 && height >= 1.0) {
        return None;
    }

    let layout = width.clamp(PANE_WIDTHS.0, PANE_WIDTHS.1);
    let shape = (height / width).clamp(PANE_SHAPES.0, PANE_SHAPES.1);

    Some((layout.round() as u64, (layout * shape).round() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_header_length_header_and_picture() {
        let frame = Frame {
            page: "T1".into(),
            width: 1280.0,
            height: 800.0,
            jpeg: vec![0xFF, 0xD8],
        };
        let message = frame.encode();

        let length = u32::from_le_bytes(message[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&message[4..4 + length]).unwrap();
        assert_eq!(header["page"], "T1");
        assert_eq!(header["width"], 1280.0);
        assert_eq!(&message[4 + length..], &[0xFF, 0xD8]);
    }

    #[test]
    fn the_panes_messages_parse() {
        let mouse: ViewInput =
            serde_json::from_str(r#"{"type":"mouse","event":"down","x":1,"y":2,"clickCount":1}"#).unwrap();
        assert!(matches!(mouse, ViewInput::Mouse(MouseInput { ref event, .. }) if event == "down"));

        let dialog: ViewInput = serde_json::from_str(r#"{"type":"dialog","accept":true}"#).unwrap();
        assert_eq!(
            dialog,
            ViewInput::Dialog {
                accept: true,
                text: None
            }
        );
    }

    fn held(copy: Option<(f64, &str)>, held: Option<&str>) -> Clipboard {
        Clipboard {
            copy: copy.map(|(id, text)| Copied { id, text: text.into() }),
            held: held.map(str::to_string),
        }
    }

    #[test]
    fn a_copy_or_a_changed_clipboard_is_what_an_input_copied() {
        let before = held(Some((1.0, "old")), Some("old"));

        assert_eq!(
            held(Some((1.0, "old")), Some("old")).copied_since(&before),
            None,
            "nothing new"
        );
        assert_eq!(
            held(Some((2.0, "old")), Some("old")).copied_since(&before).as_deref(),
            Some("old"),
            "copying the same text again copies it again"
        );
        assert_eq!(
            held(Some((1.0, "old")), Some("written"))
                .copied_since(&before)
                .as_deref(),
            Some("written"),
            "a page's writeText fires no copy event"
        );
        assert_eq!(
            held(Some((2.0, "selected")), None)
                .copied_since(&held(None, None))
                .as_deref(),
            Some("selected"),
            "a page that cannot read its clipboard tells what it copied"
        );
        assert_eq!(
            held(None, Some("old")).copied_since(&before),
            None,
            "a new document has copied nothing"
        );
        assert_eq!(held(Some((2.0, "")), None).copied_since(&before), None);
        assert_eq!(
            held(Some((2.0, "")), Some("old")).copied_since(&before),
            None,
            "copying with nothing selected copies nothing"
        );
    }

    #[test]
    fn clicks_and_shortcuts_may_copy_and_typing_does_not() {
        let control = ViewInput::Key(KeyInput {
            modifiers: 2,
            ..match key("down") {
                ViewInput::Key(key) => key,
                _ => unreachable!(),
            }
        });

        assert!(may_copy(&mouse("up", 1.0, None, 0)));
        assert!(may_copy(&control));
        assert!(!may_copy(&mouse("down", 1.0, None, 0)));
        assert!(!may_copy(&mouse("move", 1.0, None, 0)));
        assert!(!may_copy(&key("down")));
        assert!(!may_copy(&ViewInput::Text { text: "a".into() }));
    }

    #[test]
    fn notices_are_tagged_json() {
        let copied = serde_json::to_value(Notice::Copied { text: "hi".into() }).unwrap();
        assert_eq!(copied, serde_json::json!({ "type": "copied", "text": "hi" }));
        let cursor = serde_json::to_value(Notice::Cursor { cursor: "text".into() }).unwrap();
        assert_eq!(cursor, serde_json::json!({ "type": "cursor", "cursor": "text" }));
    }

    fn mouse(event: &str, x: f64, delta_y: Option<f64>, modifiers: u8) -> ViewInput {
        ViewInput::Mouse(MouseInput {
            event: event.into(),
            x,
            y: 10.0,
            button: None,
            click_count: None,
            delta_x: None,
            delta_y,
            modifiers,
            buttons: 0,
        })
    }

    fn key(event: &str) -> ViewInput {
        ViewInput::Key(KeyInput {
            event: event.into(),
            key: "a".into(),
            code: "KeyA".into(),
            key_code: 65,
            modifiers: 0,
        })
    }

    #[test]
    fn queued_moves_fold_to_the_last_and_everything_else_keeps_its_place() {
        let batch = vec![
            mouse("move", 1.0, None, 0),
            mouse("move", 2.0, None, 0),
            mouse("down", 2.0, None, 0),
            mouse("move", 3.0, None, 0),
            mouse("move", 4.0, None, 0),
            mouse("up", 4.0, None, 0),
            key("down"),
            key("up"),
            mouse("move", 5.0, None, 0),
        ];

        let folded = coalesce(batch);

        assert_eq!(
            folded,
            [
                mouse("move", 2.0, None, 0),
                mouse("down", 2.0, None, 0),
                mouse("move", 4.0, None, 0),
                mouse("up", 4.0, None, 0),
                key("down"),
                key("up"),
                mouse("move", 5.0, None, 0),
            ]
        );
    }

    #[test]
    fn queued_wheel_turns_add_up_and_a_modifier_starts_a_new_turn() {
        let batch = vec![
            mouse("wheel", 1.0, Some(40.0), 0),
            mouse("move", 2.0, None, 0),
            mouse("wheel", 3.0, Some(60.0), 0),
            mouse("wheel", 3.0, Some(-120.0), 2),
        ];

        let folded = coalesce(batch);

        let sum = MouseInput {
            delta_x: Some(0.0),
            ..match mouse("wheel", 3.0, Some(100.0), 0) {
                ViewInput::Mouse(mouse) => mouse,
                _ => unreachable!(),
            }
        };
        assert_eq!(
            folded,
            [
                mouse("move", 2.0, None, 0),
                ViewInput::Mouse(sum),
                mouse("wheel", 3.0, Some(-120.0), 2),
            ],
            "the move first, since the run ended turning"
        );
    }

    #[test]
    fn a_pane_gets_a_desktop_wide_page_in_its_own_shape() {
        assert_eq!(pane_viewport(640.0, 800.0), Some((1280, 1600)));
        assert_eq!(pane_viewport(1600.0, 900.0), Some((1600, 900)));
        assert_eq!(pane_viewport(400.0, 4000.0), Some((1280, 3200)));
        assert_eq!(pane_viewport(5000.0, 2500.0), Some((3840, 1920)));
        assert_eq!(pane_viewport(0.0, 800.0), None);
        assert_eq!(pane_viewport(f64::NAN, 800.0), None);
        assert_eq!(pane_viewport(f64::INFINITY, 800.0), None);
    }
}
