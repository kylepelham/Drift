use super::*;

#[test]
fn the_mode_is_what_was_asked_then_the_agents_default_then_the_foreground() {
    assert_eq!(resolve_mode(Some(true), Some(false), true), Ok((Mode::Background, "requested")));
    assert_eq!(resolve_mode(Some(false), Some(true), true), Ok((Mode::Foreground, "requested")));
    assert_eq!(resolve_mode(None, Some(true), true), Ok((Mode::Background, "agent default")));
    assert_eq!(resolve_mode(None, Some(false), true), Ok((Mode::Foreground, "agent default")));
    assert_eq!(resolve_mode(None, None, true), Ok((Mode::Foreground, "default")));
    assert!(resolve_mode(Some(true), None, false).unwrap_err().contains("turned off"), "an explicit request is refused, not quietly changed");
    assert_eq!(resolve_mode(None, Some(true), false), Ok((Mode::Foreground, "background turned off")));
}
