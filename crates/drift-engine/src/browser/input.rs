//! Real input for a page: mouse and key events as the DevTools Protocol takes them, from the agent's `press`
//! and from the pane, where the user types and clicks.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

const ALT: u8 = 1;
const CONTROL: u8 = 2;
const META: u8 = 4;
const SHIFT: u8 = 8;

/// A mouse event from the pane, in the page's CSS pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MouseInput {
    /// `down`, `up`, `move` or `wheel`.
    pub event: String,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub button: Option<String>,
    #[serde(default)]
    pub click_count: Option<u32>,
    #[serde(default)]
    pub delta_x: Option<f64>,
    #[serde(default)]
    pub delta_y: Option<f64>,
    #[serde(default)]
    pub modifiers: u8,
}

/// A key event from the pane, as a browser's `KeyboardEvent` describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyInput {
    /// `down` or `up`.
    pub event: String,
    pub key: String,
    pub code: String,
    #[serde(default)]
    pub key_code: u32,
    #[serde(default)]
    pub modifiers: u8,
}

/// One key as the protocol wants it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Key {
    key: String,
    code: String,
    key_code: u32,
    /// What it types, if anything.
    text: Option<String>,
}

/// Keys named by the agent, with the code and virtual key code Windows gives them.
const NAMED: &[(&str, &str, u32, Option<&str>)] = &[
    ("Enter", "Enter", 13, Some("\r")),
    ("Tab", "Tab", 9, None),
    ("Backspace", "Backspace", 8, None),
    ("Delete", "Delete", 46, None),
    ("Escape", "Escape", 27, None),
    ("ArrowUp", "ArrowUp", 38, None),
    ("ArrowDown", "ArrowDown", 40, None),
    ("ArrowLeft", "ArrowLeft", 37, None),
    ("ArrowRight", "ArrowRight", 39, None),
    ("Home", "Home", 36, None),
    ("End", "End", 35, None),
    ("PageUp", "PageUp", 33, None),
    ("PageDown", "PageDown", 34, None),
    ("Insert", "Insert", 45, None),
    ("Space", "Space", 32, Some(" ")),
];

/// The code points WebDriver reserves for keys that type nothing.
const WEBDRIVER_KEYS: &[(&str, &str)] = &[
    ("Enter", "\u{E007}"),
    ("Tab", "\u{E004}"),
    ("Backspace", "\u{E003}"),
    ("Delete", "\u{E017}"),
    ("Escape", "\u{E00C}"),
    ("ArrowLeft", "\u{E012}"),
    ("ArrowUp", "\u{E013}"),
    ("ArrowRight", "\u{E014}"),
    ("ArrowDown", "\u{E015}"),
    ("Home", "\u{E011}"),
    ("End", "\u{E010}"),
    ("PageUp", "\u{E00E}"),
    ("PageDown", "\u{E00F}"),
    ("Insert", "\u{E016}"),
    ("Shift", "\u{E008}"),
    ("Control", "\u{E009}"),
    ("Alt", "\u{E00A}"),
    ("Meta", "\u{E03D}"),
];

/// The WebDriver keys to hold for `modifiers`, in the order they go down.
pub(crate) fn webdriver_modifiers(modifiers: u8) -> Vec<&'static str> {
    [
        (CONTROL, "\u{E009}"),
        (SHIFT, "\u{E008}"),
        (ALT, "\u{E00A}"),
        (META, "\u{E03D}"),
    ]
    .into_iter()
    .filter(|(bit, _)| modifiers & bit != 0)
    .map(|(_, key)| key)
    .collect()
}

/// `Control+Shift+K`, `Enter`, `a`: the key and the modifiers held with it.
pub(crate) fn parse_chord(chord: &str) -> Result<(Key, u8), String> {
    let parts: Vec<&str> = chord.split('+').filter(|part| !part.is_empty()).collect();
    let Some((last, held)) = parts.split_last() else {
        return Err("`key` is empty".into());
    };

    let mut modifiers = 0;
    for name in held {
        modifiers |= match name.to_ascii_lowercase().as_str() {
            "alt" => ALT,
            "control" | "ctrl" => CONTROL,
            "meta" | "cmd" | "command" => META,
            "shift" => SHIFT,
            other => return Err(format!("`{other}` is not a modifier; use Alt, Control, Meta or Shift")),
        };
    }

    let key = named(last).or_else(|| character(last)).ok_or_else(|| {
        format!("`{last}` is not a key; use one character or a name such as Enter, Tab, Escape, ArrowDown, F5")
    })?;

    Ok((key, modifiers))
}

fn named(name: &str) -> Option<Key> {
    if let Some(&(key, code, key_code, text)) = NAMED.iter().find(|(key, ..)| key.eq_ignore_ascii_case(name)) {
        let key = if key == "Space" { " " } else { key };
        return Some(Key {
            key: key.into(),
            code: code.into(),
            key_code,
            text: text.map(str::to_string),
        });
    }

    let number: u32 = name.strip_prefix(['F', 'f'])?.parse().ok()?;
    (1..=12).contains(&number).then(|| Key {
        key: format!("F{number}"),
        code: format!("F{number}"),
        key_code: 111 + number,
        text: None,
    })
}

fn character(text: &str) -> Option<Key> {
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }

    let upper = character.to_ascii_uppercase();
    let (code, key_code) = match character {
        'a'..='z' | 'A'..='Z' => (format!("Key{upper}"), u32::from(upper)),
        '0'..='9' => (format!("Digit{character}"), u32::from(character)),
        _ => (String::new(), 0),
    };

    Some(Key {
        key: character.to_string(),
        code,
        key_code,
        text: Some(character.to_string()),
    })
}

impl Key {
    /// The pane's key as the protocol's: printable keys type their character unless Control or Meta is held.
    pub(crate) fn from_input(input: &KeyInput) -> Self {
        let text = match input.key.as_str() {
            "Enter" => Some("\r".to_string()),
            key if key.chars().count() == 1 => Some(key.to_string()),
            _ => None,
        };

        Self {
            key: input.key.clone(),
            code: input.code.clone(),
            key_code: input.key_code,
            text,
        }
    }

    /// The key's name and the virtual key code Windows gives it, as an IE page's script sees a key.
    pub(crate) fn name_and_code(&self) -> (String, u32) {
        (self.key.clone(), self.key_code)
    }

    /// The key as WebDriver input names it: the character, or the code point WebDriver reserves for it.
    pub(crate) fn webdriver_value(&self) -> String {
        if let Some((_, code)) = WEBDRIVER_KEYS.iter().find(|(name, _)| *name == self.key) {
            return (*code).to_string();
        }
        if let Some(number) = self.key.strip_prefix('F').and_then(|number| number.parse::<u32>().ok())
            && (1..=12).contains(&number)
        {
            return char::from_u32(0xE030 + number).map(String::from).unwrap_or_default();
        }

        self.key.clone()
    }

    /// The down and up events for this key with `modifiers` held.
    pub(crate) fn events(&self, modifiers: u8) -> [Value; 2] {
        [self.event("down", modifiers), self.event("up", modifiers)]
    }

    pub(crate) fn event(&self, direction: &str, modifiers: u8) -> Value {
        // A shortcut such as Control+A edits rather than types.
        let text = self.text.as_deref().filter(|_| modifiers & (CONTROL | META) == 0);
        let kind = match (direction, text) {
            ("up", _) => "keyUp",
            (_, Some(_)) => "keyDown",
            (_, None) => "rawKeyDown",
        };

        let mut event = json!({
            "type": kind,
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.key_code,
            "nativeVirtualKeyCode": self.key_code,
            "modifiers": modifiers,
        });
        if let (Some(text), "keyDown") = (text, kind) {
            event["text"] = json!(text);
            event["unmodifiedText"] = json!(text);
        }

        event
    }
}

impl MouseInput {
    /// The protocol's `Input.dispatchMouseEvent` parameters, or `None` for an event it has no name for.
    pub(crate) fn event(&self) -> Option<Value> {
        let kind = match self.event.as_str() {
            "down" => "mousePressed",
            "up" => "mouseReleased",
            "move" => "mouseMoved",
            "wheel" => "mouseWheel",
            _ => return None,
        };
        let button = match self.event.as_str() {
            "move" | "wheel" => "none",
            _ => self.button.as_deref().unwrap_or("left"),
        };

        Some(json!({
            "type": kind,
            "x": self.x,
            "y": self.y,
            "button": button,
            "clickCount": self.click_count.unwrap_or(1),
            "deltaX": self.delta_x.unwrap_or(0.0),
            "deltaY": self.delta_y.unwrap_or(0.0),
            "modifiers": self.modifiers,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_name_their_key_and_modifiers() {
        let (key, modifiers) = parse_chord("Control+Shift+k").unwrap();
        assert_eq!(modifiers, CONTROL | SHIFT);
        assert_eq!(key.code, "KeyK");
        assert_eq!(key.key_code, 75);

        let (enter, none) = parse_chord("enter").unwrap();
        assert_eq!((enter.key.as_str(), enter.key_code, none), ("Enter", 13, 0));
        assert_eq!(parse_chord("F5").unwrap().0.key_code, 116);
        assert!(parse_chord("Hyper+a").is_err());
        assert!(parse_chord("Banana").is_err());
    }

    #[test]
    fn webdriver_names_keys_by_their_reserved_code_points() {
        assert_eq!(parse_chord("Enter").unwrap().0.webdriver_value(), "\u{E007}");
        assert_eq!(parse_chord("F5").unwrap().0.webdriver_value(), "\u{E035}");
        assert_eq!(parse_chord("q").unwrap().0.webdriver_value(), "q");
        assert_eq!(webdriver_modifiers(CONTROL | SHIFT), ["\u{E009}", "\u{E008}"]);
    }

    #[test]
    fn a_shortcut_does_not_type_its_letter() {
        let (key, _) = parse_chord("a").unwrap();

        assert_eq!(key.event("down", 0)["text"], "a");
        assert_eq!(key.event("down", 0)["type"], "keyDown");
        assert!(key.event("down", CONTROL).get("text").is_none());
        assert_eq!(key.event("down", CONTROL)["type"], "rawKeyDown");
        assert_eq!(key.event("up", 0)["type"], "keyUp");
    }

    #[test]
    fn the_panes_mouse_maps_onto_the_protocols() {
        let click = MouseInput {
            event: "down".into(),
            x: 10.0,
            y: 20.0,
            button: None,
            click_count: Some(2),
            delta_x: None,
            delta_y: None,
            modifiers: 0,
        };
        let event = click.event().unwrap();

        assert_eq!(event["type"], "mousePressed");
        assert_eq!(event["button"], "left");
        assert_eq!(event["clickCount"], 2);
        assert!(
            MouseInput {
                event: "drag".into(),
                ..click
            }
            .event()
            .is_none()
        );
    }
}
