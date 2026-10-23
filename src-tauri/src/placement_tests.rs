use super::*;

const LEFT: Screen = Screen {
    x: 0,
    y: 0,
    width: 2560,
    height: 1400,
    scale: 1.0,
};
const RIGHT: Screen = Screen {
    x: 2560,
    y: 0,
    width: 1920,
    height: 1040,
    scale: 1.5,
};

fn at(x: i32, y: i32, width: u32, height: u32) -> Placement {
    Placement {
        x,
        y,
        width,
        height,
        maximized: false,
    }
}

#[test]
fn a_spot_on_a_screen_that_is_still_there_comes_back_as_it_was() {
    let on_right = Placement {
        maximized: true,
        ..at(2900, 120, 1500, 950)
    };

    assert_eq!(fit(on_right, &[LEFT, RIGHT]), Some(on_right));
    assert_eq!(fit(at(100, 80, 1700, 1100), &[LEFT]), Some(at(100, 80, 1700, 1100)));
    assert_eq!(
        fit(at(-200, 300, 1400, 900), &[LEFT]),
        Some(at(-200, 300, 1400, 900)),
        "partly off the left edge, but still draggable"
    );
}

#[test]
fn a_spot_on_a_screen_that_is_gone_or_out_of_reach_is_dropped() {
    assert_eq!(
        fit(at(2900, 120, 1500, 950), &[LEFT]),
        None,
        "the right screen was unplugged"
    );
    assert_eq!(
        fit(at(400, -600, 1400, 900), &[LEFT]),
        None,
        "top edge above every screen"
    );
    assert_eq!(
        fit(at(2500, 300, 1400, 900), &[LEFT]),
        None,
        "only a sliver of the top edge shows"
    );
    assert_eq!(fit(at(-32000, -32000, 1400, 900), &[LEFT, RIGHT]), None, "minimized");
    assert_eq!(fit(at(100, 100, 0, 900), &[LEFT]), None);
}

#[test]
fn a_window_bigger_than_its_screen_shrinks_onto_it() {
    let smaller = Screen {
        width: 1600,
        height: 860,
        ..LEFT
    };

    assert_eq!(fit(at(300, 200, 2400, 1300), &[smaller]), Some(at(0, 0, 1600, 860)));
}
