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

/// Applies the pane's input in the order it was sent. Moves and wheel turns that queued while the browser was busy
/// are folded together, so a browser that answers each one slowly (Firefox) never falls behind the pointer.
pub(crate) async fn apply_inputs(driver: Arc<dyn Driver>, mut queued: mpsc::UnboundedReceiver<ViewInput>) {
    while let Some(first) = queued.recv().await {
        let mut batch = vec![first];
        while let Ok(next) = queued.try_recv() {
            batch.push(next);
        }

        for input in coalesce(batch) {
            let _ = driver.input(input).await;
        }
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
