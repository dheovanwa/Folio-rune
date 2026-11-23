//! Distraction-free mode: the titlebar steps out of the way while the reader reads, and comes
//! back the moment they reach for it.
//!
//! The behaviour is a state machine with one `enum`, not a pile of booleans, so the transitions
//! are readable in one place and testable without a window:
//!
//! - `Shown`: the bar is there and the reader is using the app.
//! - `Resting`: the reader has been still for [`IDLE_SECS`]; nothing hides yet, because an eye
//!   travelling down a page is subtle and a bar that vanished the moment a timer ended would
//!   flicker on a page that is being read slowly.
//! - `Hidden`: the bar is gone, and the article has its 56 points. Any pointer move, wheel, key
//!   or press brings it back.
//! - `Pinned`: the reader asked for the bar to stay (a panel that needs it is open, or they
//!   pinned it). It never auto-hides while pinned.
//!
//! Who drives it: handlers call [`Chrome::nudge`] on input and the `*_pin`/`panel` methods on
//! intent, and `poll` calls [`Chrome::advance`] with the frame clock. Elements cannot change
//! state, but a `poll` can, so the idle timer lives there and asks for exactly the wake it
//! needs: a still reader's window stops drawing instead of polling.

/// Seconds of stillness before the bar rests, then the same again before it hides.
pub const IDLE_SECS: f64 = 3.0;

/// Where the auto-hide behaviour is. See the module docs for what each one means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Visible, and the reader is active.
    #[default]
    Shown,
    /// Visible, but the reader has been still long enough that the next idle step hides it.
    Resting,
    /// Out of the way. Any input brings it back.
    Hidden,
    /// Held open: a panel that needs it is open, or the reader asked.
    Pinned,
}

/// What woke the state machine up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Pointer moved, wheel turned, a key went down or something was pressed: the reader is here.
    Activity,
    /// The reader may have stopped: the state machine's own timer says the idle window ran out.
    Idle,
    /// The reader asked for the bar to stay (a pin toggle).
    Pin(bool),
    /// A panel that needs the bar opened or closed (settings, contents, the address field).
    Panel(bool),
    /// A new article opened, which starts with the chrome showing.
    Opened,
}

/// One step of the machine. Returns the new state and whether the bar's visibility changed, so
/// the caller knows when the view must run.
pub fn step(state: State, event: Event, enabled: bool) -> (State, bool) {
    if !enabled {
        // Distraction-free mode is off: the bar is simply always there, and panels behave.
        return (State::Shown, false);
    }
    let was_visible = visible(state);
    let next = match (state, event) {
        // Any activity shows the bar, whatever it was doing. Only a pin holds it.
        (State::Pinned, Event::Activity) => State::Pinned,
        (_, Event::Activity) => State::Shown,
        (_, Event::Pin(true)) => State::Pinned,
        (State::Pinned, Event::Pin(false)) => State::Shown,
        (_, Event::Pin(false)) => state,
        // A panel that needs the chrome opens it; closing it returns to resting, not to
        // `Shown`, so a reader who closes Settings does not have to sit still again.
        (_, Event::Panel(true)) => State::Pinned,
        (State::Pinned, Event::Panel(false)) => State::Resting,
        (_, Event::Panel(false)) => state,
        (_, Event::Opened) => State::Shown,
        // Stillness: only an idle `Shown` moves on. `Resting` is the one extra step that stops a
        // page being read slowly from flickering, and `Pinned` never hides.
        (State::Shown, Event::Idle) => State::Resting,
        (State::Resting, Event::Idle) => State::Hidden,
        (state, Event::Idle) => state,
    };
    (next, visible(next) != was_visible)
}

/// Whether the bar shows in this state.
pub fn visible(state: State) -> bool {
    !matches!(state, State::Hidden)
}

/// What a clock tick decided: the state after it, when the clock should next run, and whether
/// the view must run again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub state: State,
    /// The instant stillness began, on the app clock.
    pub since: Option<f64>,
    /// The instant the next tick wants, if any.
    pub wake_at: Option<f64>,
    /// Whether the bar's visibility changed.
    pub changed: bool,
}

/// The idle clock as a pure function of the state, when stillness began, and now.
///
/// Keeping it pure lets a test drive minutes of a reader's silence in microseconds, and keeps
/// the wake request honest: a state that needs no timer asks for none, so an idle window stops
/// drawing instead of polling.
pub fn tick(state: State, since: Option<f64>, now: f64, enabled: bool) -> Plan {
    if !enabled || state == State::Pinned {
        return Plan { state, since: None, wake_at: None, changed: false };
    }
    let Some(start) = since else {
        return Plan { state, since: Some(now), wake_at: Some(now + IDLE_SECS), changed: false };
    };
    if now - start < IDLE_SECS {
        return Plan { state, since, wake_at: Some(start + IDLE_SECS), changed: false };
    }
    let (next, changed) = step(state, Event::Idle, enabled);
    let wake_at = if next == State::Resting { Some(now + IDLE_SECS) } else { None };
    Plan { state: next, since: Some(now), wake_at, changed }
}

/// The whole behaviour in one value the app holds.
///
/// Every method returns whether the *view* must run again. A step that only moves the clock
/// returns false: the reader's stillness should not cost a commit.
#[derive(Clone, Debug)]
pub struct Chrome {
    state: State,
    enabled: bool,
    /// When the current stillness began, on the app clock. `None` means "the reader just acted",
    /// and the next tick starts counting.
    still_since: Option<f64>,
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome { state: State::Shown, enabled: true, still_since: None }
    }
}

impl Chrome {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the bar is showing, which is what decides whether it is drawn at all.
    pub fn is_visible(&self) -> bool {
        visible(self.state)
    }

    pub fn is_pinned(&self) -> bool {
        self.state == State::Pinned
    }

    /// Turn distraction-free mode on or off. Off means the bar never hides, which is what a
    /// reader who is still learning the keys wants.
    pub fn set_enabled(&mut self, on: bool) -> bool {
        let changed = on != self.enabled || !self.is_visible();
        self.enabled = on;
        self.state = State::Shown;
        self.still_since = None;
        changed
    }

    fn go(&mut self, event: Event) -> bool {
        let (next, changed) = step(self.state, event, self.enabled);
        self.state = next;
        // `Idle` must not restart the clock the timer just read, or it would never fire.
        if !matches!(event, Event::Idle) {
            self.still_since = None;
        }
        changed
    }

    /// The reader did something. No clock needed: the next tick starts the stillness window.
    pub fn nudge(&mut self) -> bool {
        self.go(Event::Activity)
    }

    /// Pin the bar open, or release it.
    pub fn pin(&mut self, on: bool) -> bool {
        self.go(Event::Pin(on))
    }

    /// A panel that needs the bar opened or closed.
    pub fn panel(&mut self, open: bool) -> bool {
        self.go(Event::Panel(open))
    }

    /// A new article is opening.
    pub fn opened(&mut self) -> bool {
        self.go(Event::Opened)
    }

    /// Advance the idle clock: one [`tick`], applied, and a wake requested for the next instant
    /// the machine needs.
    pub fn advance(&mut self, now: f64, tx: &mut rune::Tx) -> bool {
        let plan = tick(self.state, self.still_since, now, self.enabled);
        self.state = plan.state;
        self.still_since = plan.since;
        if let Some(due) = plan.wake_at {
            tx.wake_at(due);
        }
        plan.changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(state: State, event: Event) -> State {
        step(state, event, true).0
    }

    #[test]
    fn stillness_takes_two_idle_steps_so_a_slow_reader_does_not_flicker() {
        let resting = go(State::Shown, Event::Idle);
        assert_eq!(resting, State::Resting, "one idle step only rests");
        assert!(visible(resting), "and the bar is still there");
        let hidden = go(resting, Event::Idle);
        assert_eq!(hidden, State::Hidden);
        assert!(!visible(hidden));
    }

    #[test]
    fn any_activity_brings_the_bar_back() {
        assert_eq!(go(State::Hidden, Event::Activity), State::Shown);
        assert_eq!(go(State::Resting, Event::Activity), State::Shown);
    }

    #[test]
    fn a_pin_survives_idleness_and_activity() {
        let pinned = go(State::Shown, Event::Pin(true));
        assert_eq!(pinned, State::Pinned);
        assert_eq!(go(pinned, Event::Idle), State::Pinned);
        assert_eq!(go(pinned, Event::Activity), State::Pinned);
        assert_eq!(go(pinned, Event::Pin(false)), State::Shown);
    }

    #[test]
    fn closing_a_panel_rests_rather_than_showing() {
        let panel = go(State::Hidden, Event::Panel(true));
        assert_eq!(panel, State::Pinned, "a panel opens the bar it needs");
        assert_eq!(go(panel, Event::Panel(false)), State::Resting);
    }

    #[test]
    fn the_mode_off_never_hides() {
        assert_eq!(step(State::Hidden, Event::Idle, false), (State::Shown, true));
        assert_eq!(step(State::Shown, Event::Idle, false), (State::Shown, false));
    }

    #[test]
    fn a_new_article_shows_the_chrome() {
        assert_eq!(go(State::Hidden, Event::Opened), State::Shown);
    }

    #[test]
    fn the_clock_rests_then_hides_and_only_asks_for_the_wakes_it_needs() {
        let first = tick(State::Shown, None, 0.0, true);
        assert_eq!(first.since, Some(0.0), "the first tick only starts the clock");
        assert_eq!(first.wake_at, Some(IDLE_SECS));
        assert!(!first.changed);

        let early = tick(State::Shown, Some(0.0), IDLE_SECS - 0.1, true);
        assert_eq!(early.state, State::Shown, "not yet");
        assert_eq!(early.wake_at, Some(IDLE_SECS));

        let rested = tick(State::Shown, Some(0.0), IDLE_SECS, true);
        assert_eq!(rested.state, State::Resting);
        assert!(rested.changed, "the view runs: the bar leaves");
        assert_eq!(rested.wake_at, Some(IDLE_SECS * 2.0), "and the second half is scheduled");

        let hidden = tick(State::Resting, rested.since, IDLE_SECS * 2.0, true);
        assert_eq!(hidden.state, State::Hidden);
        assert!(hidden.changed);
        assert_eq!(hidden.wake_at, None, "nothing left to wait for: the window can rest");
    }

    #[test]
    fn activity_in_the_middle_of_the_window_resets_it() {
        let mut c = Chrome::default();
        let mut tx = rune::Tx::default();
        c.advance(0.0, &mut tx);
        assert!(c.nudge(), "a hidden bar comes back");
        assert!(!c.advance(1.5, &mut tx), "the clock restarted at the activity");
        assert_eq!(c.state(), State::Shown);
    }

    #[test]
    fn a_nudge_while_shown_costs_no_view_run() {
        let mut c = Chrome::default();
        assert!(!c.nudge(), "typing in a shown bar must not commit");
        assert_eq!(c.state(), State::Shown);
    }

    #[test]
    fn a_pinned_bar_asks_for_no_timer() {
        let plan = tick(State::Pinned, Some(0.0), 1_000.0, true);
        assert_eq!(plan.state, State::Pinned);
        assert_eq!(plan.wake_at, None);
    }

    #[test]
    fn the_mode_off_leaves_the_bar_alone_even_when_the_clock_runs() {
        let mut c = Chrome::default();
        c.set_enabled(false);
        let plan = tick(State::Hidden, Some(0.0), 1_000.0, false);
        assert_eq!(plan.state, State::Shown);
        assert_eq!(plan.wake_at, None);
        assert!(c.is_visible());
    }
}
