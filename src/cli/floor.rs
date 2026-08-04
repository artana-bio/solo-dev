//! The `demo` assembly-line animation.
//!
//! This module is presentation only: it never reads a control repository and
//! never runs a gate. It renders a fixed, fictional cycle — three invented
//! cards, none sharing an identifier with anything in this project's own
//! history — as an assembly line with six stations named after the real
//! command each one stands in for. The point is to make the shape of the
//! harness (linear stations, a station that can reject, rework looping a
//! card backwards, a weld step that gives the *combination* its own gate, a
//! final promotion) visible at a glance, not to simulate real command
//! behavior.
//!
//! [`frames`] is a pure function: given a width, it returns the complete,
//! deterministic frame sequence up front. [`play`] is the only effectful
//! part, and it takes its delay as a parameter so tests can drive it with
//! [`RecordingSink`] and a zero delay instead of waiting through a real
//! playback.

use std::{fmt::Write as _, io, time::Duration};

/// One rendered frame: a fixed number of complete lines, no trailing newline.
pub type Frame = Vec<String>;

/// A destination for rendered frames.
///
/// Production writes to a real terminal ([`TerminalSink`]); tests collect
/// frames in memory ([`RecordingSink`]) so the frame sequence is assertable
/// without a TTY and without waiting through real playback delays.
pub trait FrameSink {
    /// Displays one frame, replacing whatever this sink showed before.
    fn present(&mut self, frame: &Frame);
}

/// Plays a frame sequence, pacing each frame by `frame_delay`.
///
/// No delay follows the final frame, so a caller chaining output after
/// `play` returns does not pay an extra, invisible wait.
pub fn play(frames: &[Frame], sink: &mut dyn FrameSink, frame_delay: Duration) {
    let mut remaining = frames.len();
    for frame in frames {
        sink.present(frame);
        remaining -= 1;
        if remaining > 0 && !frame_delay.is_zero() {
            std::thread::sleep(frame_delay);
        }
    }
}

/// Writes frames to a real terminal via in-place redraw.
///
/// Deliberately does not use the alternate-screen buffer. That would need to
/// be restored on exit, and a `Drop` guard cannot run on an unhandled
/// `SIGINT` — catching that portably needs a signal handler, which this crate
/// has no dependency for and which is not worth adding for a cosmetic
/// feature. In-place redraw (move the cursor up, clear each line, redraw —
/// the same technique progress bars use) has no mode to restore: the worst
/// case of an interrupted run is a partially drawn frame left on screen,
/// never a terminal that stops accepting input. The cursor is still hidden
/// and restored through a `Drop` guard, but that failing open (a cursor left
/// hidden) is purely cosmetic and corrects itself at the next shell prompt.
pub struct TerminalSink<W: io::Write> {
    out: W,
    drawn_lines: usize,
}

impl<W: io::Write> TerminalSink<W> {
    /// Wraps a writer and hides the cursor for the duration of playback.
    pub fn new(mut out: W) -> Self {
        let _ = write!(out, "\x1b[?25l");
        let _ = out.flush();
        Self {
            out,
            drawn_lines: 0,
        }
    }
}

impl<W: io::Write> FrameSink for TerminalSink<W> {
    fn present(&mut self, frame: &Frame) {
        if self.drawn_lines > 0 {
            let _ = write!(self.out, "\x1b[{}A", self.drawn_lines);
        }
        for line in frame {
            let _ = write!(self.out, "\x1b[2K{line}\r\n");
        }
        let _ = self.out.flush();
        self.drawn_lines = frame.len();
    }
}

impl<W: io::Write> Drop for TerminalSink<W> {
    fn drop(&mut self) {
        let _ = write!(self.out, "\x1b[?25h");
        let _ = self.out.flush();
    }
}

/// Collects presented frames in memory, for tests.
#[derive(Debug, Default)]
pub struct RecordingSink {
    /// Every frame presented so far, in order.
    pub frames: Vec<Frame>,
}

impl FrameSink for RecordingSink {
    fn present(&mut self, frame: &Frame) {
        self.frames.push(frame.clone());
    }
}

/// Discards every frame.
///
/// Used on the skip path, where nothing must ever touch the terminal: unlike
/// [`TerminalSink`], constructing this performs no I/O, so a caller that
/// already decided not to play the animation can still satisfy the sink
/// parameter without hiding and immediately re-showing the cursor for
/// nothing.
#[derive(Debug, Default)]
pub struct NullSink;

impl FrameSink for NullSink {
    fn present(&mut self, _frame: &Frame) {}
}

/// A station on the floor, named after the real command it stands in for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Station {
    /// `work start` — a card enters the floor.
    Intake,
    /// `gate run` — named gates execute here.
    Bench,
    /// `handoff create` — the card is bound to an exact candidate commit.
    Press,
    /// `review record` — independent review.
    Scan,
    /// `integration verify` — the combination gets its own gates.
    Weld,
    /// `promote` — the authority ref advances.
    Ship,
}

/// Every station, in belt order.
const STATIONS: [Station; 6] = [
    Station::Intake,
    Station::Bench,
    Station::Press,
    Station::Scan,
    Station::Weld,
    Station::Ship,
];

impl Station {
    const fn label(self) -> &'static str {
        match self {
            Self::Intake => "INTAKE",
            Self::Bench => "BENCH",
            Self::Press => "PRESS",
            Self::Scan => "SCAN",
            Self::Weld => "WELD",
            Self::Ship => "SHIP",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Intake => 0,
            Self::Bench => 1,
            Self::Press => 2,
            Self::Scan => 3,
            Self::Weld => 4,
            Self::Ship => 5,
        }
    }
}

/// The real command each station stands in for, in belt order.
///
/// Exposed for the `demo` command's JSON payload, so a machine reader learns
/// the station-to-command mapping without scraping animation text.
#[must_use]
pub fn station_commands() -> [(&'static str, &'static str); 6] {
    [
        (Station::Intake.label(), "work start"),
        (Station::Bench.label(), "gate run"),
        (Station::Press.label(), "handoff create"),
        (Station::Scan.label(), "review record"),
        (Station::Weld.label(), "integration verify"),
        (Station::Ship.label(), "promote"),
    ]
}

/// One beat of a floor script: a single thing that happens on the belt.
///
/// The demo and `cycle replay` produce the same beats from different sources
/// — a canned screenplay and the journaled event history — and one
/// interpreter turns either into frames, so the two can never drift apart
/// visually.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Beat {
    /// A card enters the floor at INTAKE.
    Arrive {
        /// The card entering.
        card: String,
    },
    /// A card moves to a station.
    Advance {
        /// The card moving.
        card: String,
        /// Where it goes.
        to: Station,
    },
    /// A gate runs at BENCH and passes.
    GatePass {
        /// The card under test.
        card: String,
        /// The gate that ran.
        gate: String,
        /// Which attempt this was, starting at 1.
        attempt: u32,
    },
    /// A gate runs at BENCH, fails, and the card is ejected to rework.
    GateFailEject {
        /// The card under test.
        card: String,
        /// The gate that failed.
        gate: String,
    },
    /// The PRESS binds a card to an exact candidate commit.
    Stamp {
        /// The card being bound.
        card: String,
        /// The short candidate SHA the tumbler locks in.
        sha: String,
    },
    /// An independent review approves the card at SCAN.
    ReviewApprove {
        /// The approved card.
        card: String,
    },
    /// A review requests changes; the card is ejected to rework.
    ReviewReject {
        /// The rejected card.
        card: String,
        /// How many findings the review recorded.
        findings: usize,
    },
    /// Cards combine at WELD and the combination passes its own gates.
    Weld {
        /// The cards being combined.
        members: Vec<String>,
        /// The short landing SHA the combination verified as.
        merged_sha: String,
    },
    /// The authority ref advances; every member lands.
    Ship {
        /// The cards landing together.
        members: Vec<String>,
        /// The authority commit before promotion.
        from: String,
        /// The landing commit it advanced to.
        to: String,
    },
    /// A card leaves the floor without landing.
    Depart {
        /// The card leaving.
        card: String,
        /// Why it left.
        reason: String,
    },
    /// Evidence did not hold; the ticker dwells on the discrepancy.
    Flash {
        /// The discrepancy, already worded for the ticker.
        text: String,
    },
    /// Something happened that has no set piece; the ticker narrates it.
    Note {
        /// The narration.
        text: String,
    },
    /// The closing hold.
    Close {
        /// The final ticker line.
        text: String,
    },
}

/// A beat with its position in real history, when it has one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimedBeat {
    /// The moment in history, already rendered for the header clock.
    ///
    /// `None` inherits the previous beat's clock; the demo's screenplay never
    /// sets one, so its header stays constant.
    pub at: Option<String>,
    /// The ordinal of the source event, for the footer's `event k/n`.
    pub progress: Option<usize>,
    /// What happens.
    pub beat: Beat,
}

impl TimedBeat {
    /// A beat with no historical position, as the demo's screenplay uses.
    #[must_use]
    pub const fn untimed(beat: Beat) -> Self {
        Self {
            at: None,
            progress: None,
            beat,
        }
    }
}

/// A complete floor screenplay: what the interpreter plays.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Script {
    /// Constant prefix of the full layout's header line.
    pub header_base: String,
    /// Constant prefix of the compact layout's header line.
    pub header_compact: String,
    /// Total source events, for the footer's `event k/n`. `None` hides it.
    pub progress_total: Option<usize>,
    /// The beats, in order.
    pub beats: Vec<TimedBeat>,
}

/// A station's current animation phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Activity {
    Idle,
    Working,
    Done,
}

/// One card riding the belt.
struct FloorCard {
    id: String,
    station: Station,
    landed: bool,
}

/// The extra line shown under the belt during a set-piece beat.
///
/// At most one of these is active at a time; the beat script is linear, so
/// there is never a need to show two at once.
enum Extra {
    /// The SHA tumbler at PRESS: hex digits lock in one at a time.
    Press {
        card: String,
        full_sha: String,
        revealed: usize,
        sealed: bool,
    },
    /// A rejected card crawling back to INTAKE.
    Rework { card: String, step: u32 },
    /// Separate cards converging into one verified artifact at WELD.
    Weld { members: Vec<String>, done: bool },
    /// The authority ref advancing at SHIP.
    Ship { from: String, to: String },
}

/// The floor's complete state at one instant.
struct FloorState {
    tick: usize,
    cards: Vec<FloorCard>,
    activity: [Activity; STATIONS.len()],
    gates_run: usize,
    reworked: usize,
    landed_today: usize,
    ticker: String,
    extra: Option<Extra>,
    header_base: String,
    header_compact: String,
    /// The history clock: the last timed beat's moment, rendered.
    clock: Option<String>,
    /// `(current event ordinal, total events)` for the footer.
    progress: Option<(usize, usize)>,
    /// How many frames the whole playback holds.
    ///
    /// `None` during the counting pass; [`frames_for`] re-renders with the
    /// count so every frame can carry the playback percentage.
    frame_total: Option<usize>,
}

impl FloorState {
    fn new(script: &Script, frame_total: Option<usize>) -> Self {
        Self {
            tick: 0,
            cards: Vec::new(),
            activity: [Activity::Idle; STATIONS.len()],
            gates_run: 0,
            reworked: 0,
            landed_today: 0,
            ticker: "the floor is quiet".to_owned(),
            extra: None,
            header_base: script.header_base.clone(),
            header_compact: script.header_compact.clone(),
            clock: None,
            progress: script.progress_total.map(|total| (0, total)),
            frame_total,
        }
    }

    /// How far through the playback this frame is, in whole percent.
    ///
    /// Frame indices are zero-based and `tick` has not yet advanced when a
    /// frame renders, so the first of `n` frames shows `100/n`-rounded-down
    /// and the last always shows 100.
    fn playback_percent(&self) -> Option<usize> {
        let total = self.frame_total?;
        if total == 0 {
            return None;
        }
        Some((((self.tick + 1) * 100) / total).min(100))
    }

    /// The right-aligned progress block for the full layout: an eight-cell
    /// bar plus the percentage, `████░░░░  43%`.
    fn playback_gauge(&self) -> Option<String> {
        const CELLS: usize = 8;
        let percent = self.playback_percent()?;
        let filled = (percent * CELLS) / 100;
        Some(format!(
            "{}{} {percent:>3}%",
            "█".repeat(filled),
            "░".repeat(CELLS - filled)
        ))
    }

    /// The compact layout's progress block: the percentage alone, because a
    /// narrow terminal has no room to spend on a bar.
    fn playback_percent_label(&self) -> Option<String> {
        self.playback_percent()
            .map(|percent| format!("{percent:>3}%"))
    }

    /// The full layout's header line: the base, plus the history clock once
    /// a timed beat has set one.
    fn header_full(&self) -> String {
        match &self.clock {
            None => self.header_base.clone(),
            Some(at) => format!("{} · {at}", self.header_base),
        }
    }

    /// The compact layout's header line.
    fn header_compact_line(&self) -> String {
        match &self.clock {
            None => self.header_compact.clone(),
            Some(at) => format!("{} · {at}", self.header_compact),
        }
    }

    fn activity(&self, station: Station) -> Activity {
        self.activity[station.index()]
    }

    fn set_activity(&mut self, station: Station, activity: Activity) {
        self.activity[station.index()] = activity;
    }

    fn card_mut(&mut self, id: &str) -> Option<&mut FloorCard> {
        self.cards.iter_mut().find(|card| card.id == id)
    }
}

/// Cycle identity shown in the header.
///
/// Deliberately not `INT-027` or `F-029`: those are real identifiers from
/// this project's own history, and reusing them here would make a scripted
/// demo look like a report about actual project state.
const DEMO_CYCLE_ID: &str = "DEMO-001";
const DEMO_BASELINE: &str = "a1b2c3d";

const CELL_WIDTH: usize = 12;
const FULL_BELT_WIDTH: usize = CELL_WIDTH * STATIONS.len();
const MARGIN: &str = "  ";
const FULL_FRAME_LINES: usize = 8;
const COMPACT_FRAME_LINES: usize = 4;
const MIN_COMPACT_WIDTH: usize = 32;
const REWORK_STEPS: u32 = 5;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn spinner_glyph(tick: usize) -> char {
    SPINNER[tick % SPINNER.len()]
}

/// Centers `text` in a field of `width` columns, or truncates with an
/// ellipsis when it does not fit. Widths are counted in `char`s: every glyph
/// this module draws with is single-width, so this is exact for the content
/// that ever reaches it.
fn fit(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        let left = (width - len) / 2;
        let right = width - len - left;
        format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
    } else if width == 0 {
        String::new()
    } else {
        let truncated: String = text.chars().take(width - 1).collect();
        format!("{truncated}…")
    }
}

/// Left-aligns `text` in a field of `width` columns, or truncates with an
/// ellipsis when it does not fit.
fn fit_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        format!("{text}{}", " ".repeat(width - len))
    } else if width == 0 {
        String::new()
    } else {
        let truncated: String = text.chars().take(width - 1).collect();
        format!("{truncated}…")
    }
}

/// Lays `left` and `right` at opposite ends of a `width`-column field.
///
/// The right side wins a collision: it is the playback gauge, and a gauge
/// that gets truncated mid-percentage reads as a wrong number, whereas a
/// header that loses its tail to an ellipsis is still recognizably the
/// header.
fn spread(left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    if right_len >= width {
        return fit_left(right, width);
    }
    // One column of breathing room between the sides.
    let left_field = width - right_len - 1;
    format!("{} {right}", fit_left(left, left_field))
}

/// A conveyor pattern that appears to crawl as `tick` advances.
fn belt_row(tick: usize, width: usize) -> String {
    const PATTERN: [char; 4] = ['═', '═', '═', '▸'];
    let phase = tick % PATTERN.len();
    (0..width)
        .map(|i| PATTERN[(i + phase) % PATTERN.len()])
        .collect()
}

/// The card label shown for `station`: the first occupant plus a count.
///
/// A comma-join of every occupant used to overflow the 12-column cell and
/// truncate into `F-901,F-902…`, which reads as a mangled identifier. Real
/// cycles put more cards at one station than a cell can spell out, so past
/// one occupant the label names the first and counts the rest: `F-901 +2`.
fn ids_at(cards: &[FloorCard], station: Station) -> String {
    let mut at_station = cards.iter().filter(|card| card.station == station);
    let Some(first) = at_station.next() else {
        return String::new();
    };
    match at_station.count() {
        0 => first.id.clone(),
        more => format!("{} +{more}", first.id),
    }
}

fn icon(station: Station, activity: Activity, tick: usize) -> &'static str {
    match (station, activity) {
        (Station::Intake, Activity::Working) => "▒▒▒▒",
        (Station::Intake, Activity::Idle | Activity::Done) => "░░░░",
        (Station::Bench, Activity::Working) => {
            if tick.is_multiple_of(2) {
                "⚙ ⚙"
            } else {
                " ⚙ "
            }
        }
        (Station::Bench | Station::Scan, Activity::Done) => "✓",
        (Station::Bench, Activity::Idle) => "⚙",
        (Station::Press, Activity::Working) => {
            if tick.is_multiple_of(2) {
                "▽"
            } else {
                "▼"
            }
        }
        (Station::Press, Activity::Done) => "▪",
        (Station::Press, Activity::Idle) => "▢",
        (Station::Scan, Activity::Working) => "◉",
        (Station::Scan, Activity::Idle) => "○",
        (Station::Weld, Activity::Working) => "⋊⋉",
        (Station::Weld, Activity::Done) => "▪▪",
        (Station::Weld, Activity::Idle) => "· ·",
        (Station::Ship, _) => "⛭",
    }
}

fn extra_line(extra: Option<&Extra>) -> String {
    match extra {
        None => String::new(),
        Some(Extra::Press {
            card,
            full_sha,
            revealed,
            sealed,
        }) => {
            if *sealed {
                format!("binding {card} ▸ {full_sha} ▪ sealed")
            } else {
                let shown: String = full_sha.chars().take(*revealed).collect();
                let filler = "▓".repeat(full_sha.chars().count().saturating_sub(*revealed));
                format!("binding {card} ▸ {shown}{filler}")
            }
        }
        Some(Extra::Rework { card, step }) => {
            let travelled = "─".repeat(*step as usize);
            let remaining = "─".repeat((REWORK_STEPS - step) as usize);
            format!("◂{travelled}[{card}]{remaining}◂ rework")
        }
        Some(Extra::Weld { members, done }) => {
            let status = if *done {
                "combined gates passed"
            } else {
                "combined gates running"
            };
            format!("weld ▸ {} ▸ {status}", members.join(" + "))
        }
        Some(Extra::Ship { from, to }) => format!("authority `main` ▸ {from} → {to}"),
    }
}

fn footer_text(state: &FloorState) -> String {
    let in_flight = state.cards.iter().filter(|card| !card.landed).count();
    let mut footer = format!(
        "{in_flight} in flight · {} gates run · {} reworked · landed today: {}",
        state.gates_run, state.reworked, state.landed_today
    );
    if let Some((current, total)) = state.progress {
        let _ = write!(footer, " · event {current}/{total}");
    }
    footer
}

fn render_full(state: &FloorState) -> Frame {
    let labels: String = STATIONS
        .iter()
        .map(|s| fit(s.label(), CELL_WIDTH))
        .collect();
    let icons: String = STATIONS
        .iter()
        .map(|s| {
            fit(
                &format!("[{}]", icon(*s, state.activity(*s), state.tick)),
                CELL_WIDTH,
            )
        })
        .collect();
    let ids: String = STATIONS
        .iter()
        .map(|s| fit(&ids_at(&state.cards, *s), CELL_WIDTH))
        .collect();

    let header = match state.playback_gauge() {
        Some(gauge) => spread(&state.header_full(), &gauge, FULL_BELT_WIDTH),
        None => fit_left(&state.header_full(), FULL_BELT_WIDTH),
    };
    let lines = vec![
        format!("{MARGIN}{header}"),
        format!("{MARGIN}{labels}"),
        format!("{MARGIN}{icons}"),
        format!("{MARGIN}{ids}"),
        format!("{MARGIN}{}▸ main", belt_row(state.tick, FULL_BELT_WIDTH)),
        format!(
            "{MARGIN}{}",
            fit_left(&extra_line(state.extra.as_ref()), FULL_BELT_WIDTH)
        ),
        format!(
            "{MARGIN}{} {}",
            spinner_glyph(state.tick),
            fit_left(&state.ticker, FULL_BELT_WIDTH.saturating_sub(2))
        ),
        format!("{MARGIN}{}", fit_left(&footer_text(state), FULL_BELT_WIDTH)),
    ];
    debug_assert_eq!(
        lines.len(),
        FULL_FRAME_LINES,
        "render_full must always emit a fixed line count"
    );
    lines
}

fn render_compact(state: &FloorState, width: usize) -> Frame {
    let width = width.max(MIN_COMPACT_WIDTH);
    let belt: String = STATIONS
        .iter()
        .map(|station| {
            let ids = ids_at(&state.cards, *station);
            if ids.is_empty() {
                station.label().to_owned()
            } else {
                format!("{}[{ids}]", station.label())
            }
        })
        .collect::<Vec<_>>()
        .join("▸");

    let header = match state.playback_percent_label() {
        Some(label) => spread(&state.header_compact_line(), &label, width),
        None => fit_left(&state.header_compact_line(), width),
    };
    let lines = vec![
        header,
        fit_left(&belt, width),
        fit_left(
            &format!("{} {}", spinner_glyph(state.tick), state.ticker),
            width,
        ),
        fit_left(&footer_text(state), width),
    ];
    debug_assert_eq!(
        lines.len(),
        COMPACT_FRAME_LINES,
        "render_compact must always emit a fixed line count"
    );
    lines
}

fn render(state: &FloorState, width: usize) -> Frame {
    if width < crate::cli::tty::MIN_FULL_WIDTH {
        render_compact(state, width)
    } else {
        render_full(state)
    }
}

/// Interprets a beat script into a growing frame sequence.
struct Director {
    state: FloorState,
    frames: Vec<Frame>,
    width: usize,
}

impl Director {
    fn new(script: &Script, width: usize, frame_total: Option<usize>) -> Self {
        Self {
            state: FloorState::new(script, frame_total),
            frames: Vec::new(),
            width,
        }
    }

    fn push_frame(&mut self) {
        self.frames.push(render(&self.state, self.width));
        self.state.tick += 1;
    }

    fn hold(&mut self, count: u32) {
        for _ in 0..count {
            self.push_frame();
        }
    }

    fn set_ticker(&mut self, text: String) {
        self.state.ticker = text;
    }

    /// Plays one timed beat: the clock and progress advance first, so every
    /// frame the beat renders already sits at its moment in history.
    fn on_beat(&mut self, timed: &TimedBeat) {
        if let Some(at) = &timed.at {
            self.state.clock = Some(at.clone());
        }
        if let (Some(ordinal), Some((_, total))) = (timed.progress, self.state.progress) {
            self.state.progress = Some((ordinal, total));
        }
        match &timed.beat {
            Beat::Arrive { card } => self.arrive(card),
            Beat::Advance { card, to } => self.advance(card, *to),
            Beat::GatePass {
                card,
                gate,
                attempt,
            } => self.run_gate_pass(card, gate, *attempt),
            Beat::GateFailEject { card, gate } => self.run_gate_fail_and_eject(card, gate),
            Beat::Stamp { card, sha } => self.stamp(card, sha),
            Beat::ReviewApprove { card } => self.review_approve(card),
            Beat::ReviewReject { card, findings } => self.review_reject(card, *findings),
            Beat::Weld {
                members,
                merged_sha,
            } => self.weld(members, merged_sha),
            Beat::Ship { members, from, to } => self.ship(members, from, to),
            Beat::Depart { card, reason } => self.depart(card, reason),
            Beat::Flash { text } => self.flash(text),
            Beat::Note { text } => self.note(text),
            Beat::Close { text } => self.close(text),
        }
    }

    fn place(&mut self, card: &str, station: Station) {
        self.state.cards.push(FloorCard {
            id: card.to_owned(),
            station,
            landed: false,
        });
    }

    fn arrive(&mut self, card: &str) {
        self.place(card, Station::Intake);
        self.set_ticker(format!("{card} entered the floor"));
        self.hold(3);
    }

    fn advance(&mut self, card: &str, to: Station) {
        self.set_ticker(format!("{card} → {}", to.label()));
        self.hold(2);
        if let Some(found) = self.state.card_mut(card) {
            found.station = to;
        }
        self.hold(2);
    }

    fn run_gate_pass(&mut self, card: &str, gate: &str, attempt: u32) {
        self.state.gates_run += 1;
        self.state.set_activity(Station::Bench, Activity::Working);
        let note = if attempt > 1 {
            format!(" (attempt {attempt})")
        } else {
            String::new()
        };
        self.set_ticker(format!("gate `{gate}` running for {card}{note}"));
        self.hold(5);
        self.state.set_activity(Station::Bench, Activity::Done);
        self.set_ticker(format!("✓ {gate} passed for {card}"));
        self.hold(2);
        self.state.set_activity(Station::Bench, Activity::Idle);
    }

    /// The shared eject: the card leaves the belt, crawls back along the
    /// rework lane, and re-enters at INTAKE. Gate failures and review
    /// rejections both end here; only the ticker that precedes it differs.
    fn rework_crawl(&mut self, card: &str) {
        self.state.reworked += 1;
        self.state.cards.retain(|existing| existing.id != card);
        self.hold(2);

        for step in 0..=REWORK_STEPS {
            self.state.extra = Some(Extra::Rework {
                card: card.to_owned(),
                step,
            });
            self.set_ticker(format!("{card} returning to INTAKE for another attempt"));
            self.push_frame();
        }
        self.state.extra = None;
        self.place(card, Station::Intake);
        self.set_ticker(format!("{card} back on the belt"));
        self.hold(2);
    }

    fn run_gate_fail_and_eject(&mut self, card: &str, gate: &str) {
        self.state.gates_run += 1;
        self.state.set_activity(Station::Bench, Activity::Working);
        self.set_ticker(format!("gate `{gate}` running for {card}"));
        self.hold(5);
        self.state.set_activity(Station::Bench, Activity::Idle);
        self.set_ticker(format!("✗ {gate} failed for {card} — ejecting"));
        self.rework_crawl(card);
    }

    fn stamp(&mut self, card: &str, sha: &str) {
        self.state.set_activity(Station::Press, Activity::Working);
        self.set_ticker(format!("binding {card} to an exact commit"));
        let len = sha.chars().count();
        for revealed in 0..=len {
            self.state.extra = Some(Extra::Press {
                card: card.to_owned(),
                full_sha: sha.to_owned(),
                revealed,
                sealed: false,
            });
            self.push_frame();
        }
        self.state.extra = Some(Extra::Press {
            card: card.to_owned(),
            full_sha: sha.to_owned(),
            revealed: len,
            sealed: true,
        });
        self.state.set_activity(Station::Press, Activity::Done);
        self.set_ticker(format!("{card} sealed at {sha}"));
        self.hold(3);
        self.state.extra = None;
        self.state.set_activity(Station::Press, Activity::Idle);
    }

    fn review_approve(&mut self, card: &str) {
        self.state.set_activity(Station::Scan, Activity::Working);
        self.set_ticker(format!("independent review recording for {card}"));
        self.hold(4);
        self.state.set_activity(Station::Scan, Activity::Done);
        self.set_ticker(format!("review approved {card}"));
        self.hold(2);
        self.state.set_activity(Station::Scan, Activity::Idle);
    }

    fn review_reject(&mut self, card: &str, findings: usize) {
        self.state.set_activity(Station::Scan, Activity::Working);
        self.set_ticker(format!("independent review recording for {card}"));
        self.hold(4);
        self.state.set_activity(Station::Scan, Activity::Idle);
        self.set_ticker(format!(
            "✗ review requested changes on {card} ({findings} finding(s)) — ejecting"
        ));
        self.rework_crawl(card);
    }

    fn weld(&mut self, members: &[String], merged_sha: &str) {
        self.state.set_activity(Station::Weld, Activity::Working);
        self.state.extra = Some(Extra::Weld {
            members: members.to_vec(),
            done: false,
        });
        self.set_ticker(format!("combining {}", members.join(", ")));
        self.hold(6);
        self.state.set_activity(Station::Weld, Activity::Done);
        self.state.extra = Some(Extra::Weld {
            members: members.to_vec(),
            done: true,
        });
        self.set_ticker(format!("combined verification passed ▸ {merged_sha}"));
        self.hold(3);
        self.state.extra = None;
        self.state.set_activity(Station::Weld, Activity::Idle);
    }

    fn ship(&mut self, members: &[String], from: &str, to: &str) {
        self.state.extra = Some(Extra::Ship {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        self.set_ticker("promoting to `main`".to_owned());
        self.hold(5);
        self.state.landed_today += members.len();
        for card in members {
            if let Some(found) = self.state.card_mut(card) {
                found.landed = true;
            }
        }
        self.set_ticker(format!("landed on `main` at {to}"));
        self.hold(3);
        self.state.extra = None;
    }

    fn depart(&mut self, card: &str, reason: &str) {
        self.state.cards.retain(|existing| existing.id != card);
        self.set_ticker(format!("{card} left the floor — {reason}"));
        self.hold(3);
    }

    fn flash(&mut self, text: &str) {
        // A discrepancy dwells twice as long as a narration: it is the one
        // line a viewer must not scroll past.
        self.set_ticker(text.to_owned());
        self.hold(6);
    }

    fn note(&mut self, text: &str) {
        self.set_ticker(text.to_owned());
        self.hold(3);
    }

    fn close(&mut self, text: &str) {
        self.set_ticker(text.to_owned());
        self.hold(24);
    }
}

/// Renders a script's complete, deterministic frame sequence at `width`
/// columns.
///
/// Pure and fast: no sleeping, no I/O. Production hands the result to
/// [`play`]; tests inspect it directly.
#[must_use]
pub fn frames_for(script: &Script, width: usize) -> Vec<Frame> {
    // Two passes: the first counts frames, the second renders with the
    // count so every frame's header can carry the playback percentage. The
    // count depends only on the beats — never on what the frames show — so
    // the passes cannot disagree, and a full render is cheap enough
    // (hundreds of small strings) that counting analytically would be
    // complexity without a payoff.
    let total = render_pass(script, width, None).len();
    render_pass(script, width, Some(total))
}

/// One interpreter run over the script.
fn render_pass(script: &Script, width: usize, frame_total: Option<usize>) -> Vec<Frame> {
    let mut director = Director::new(script, width, frame_total);
    for beat in &script.beats {
        director.on_beat(beat);
    }
    director.frames
}

/// The demo's canned screenplay: three fictional cards through the full
/// lifecycle, one gate failure with rework, one combined landing.
#[must_use]
pub fn demo_script() -> Script {
    fn advance(card: &str, to: Station) -> TimedBeat {
        TimedBeat::untimed(Beat::Advance {
            card: card.to_owned(),
            to,
        })
    }
    fn arrive(card: &str) -> TimedBeat {
        TimedBeat::untimed(Beat::Arrive {
            card: card.to_owned(),
        })
    }
    fn stamp(card: &str, sha: &str) -> TimedBeat {
        TimedBeat::untimed(Beat::Stamp {
            card: card.to_owned(),
            sha: sha.to_owned(),
        })
    }
    fn approve(card: &str) -> TimedBeat {
        TimedBeat::untimed(Beat::ReviewApprove {
            card: card.to_owned(),
        })
    }
    fn gate_pass(card: &str, gate: &str, attempt: u32) -> TimedBeat {
        TimedBeat::untimed(Beat::GatePass {
            card: card.to_owned(),
            gate: gate.to_owned(),
            attempt,
        })
    }
    let members = || vec!["F-901".to_owned(), "F-902".to_owned(), "F-903".to_owned()];

    Script {
        header_base: format!("cycle {DEMO_CYCLE_ID} · baseline {DEMO_BASELINE}"),
        header_compact: format!("{DEMO_CYCLE_ID} · {DEMO_BASELINE}"),
        progress_total: None,
        beats: vec![
            arrive("F-901"),
            advance("F-901", Station::Bench),
            gate_pass("F-901", "typecheck", 1),
            advance("F-901", Station::Press),
            arrive("F-902"),
            stamp("F-901", "7f3a91c"),
            advance("F-901", Station::Scan),
            approve("F-901"),
            advance("F-902", Station::Bench),
            TimedBeat::untimed(Beat::GateFailEject {
                card: "F-902".to_owned(),
                gate: "test-suite".to_owned(),
            }),
            arrive("F-903"),
            advance("F-902", Station::Bench),
            gate_pass("F-902", "test-suite", 2),
            advance("F-903", Station::Bench),
            gate_pass("F-903", "lint", 1),
            advance("F-902", Station::Press),
            stamp("F-902", "3c8e04d"),
            advance("F-903", Station::Press),
            stamp("F-903", "b12aa9e"),
            advance("F-902", Station::Scan),
            approve("F-902"),
            advance("F-903", Station::Scan),
            approve("F-903"),
            advance("F-901", Station::Weld),
            advance("F-902", Station::Weld),
            advance("F-903", Station::Weld),
            TimedBeat::untimed(Beat::Weld {
                members: members(),
                merged_sha: "9c1d4e0".to_owned(),
            }),
            advance("F-901", Station::Ship),
            advance("F-902", Station::Ship),
            advance("F-903", Station::Ship),
            TimedBeat::untimed(Beat::Ship {
                members: members(),
                from: "0e77aa1".to_owned(),
                to: "9c1d4e0".to_owned(),
            }),
            TimedBeat::untimed(Beat::Close {
                text: "demo complete — a scripted walkthrough; no repository was read or changed"
                    .to_owned(),
            }),
        ],
    }
}

/// Renders the complete, deterministic demo frame sequence at `width`
/// columns.
#[must_use]
pub fn frames(width: usize) -> Vec<Frame> {
    frames_for(&demo_script(), width)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- fit / fit_left -----------------------------------------------

    #[test]
    fn fit_centers_short_text_and_pads_to_exact_width() {
        let result = fit("hi", 6);
        assert_eq!(result.chars().count(), 6);
        assert_eq!(result, "  hi  ");
    }

    #[test]
    fn fit_truncates_overlong_text_with_an_ellipsis() {
        let result = fit("way too long for this cell", 6);
        assert_eq!(result.chars().count(), 6);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn fit_handles_exact_width_with_no_padding() {
        let result = fit("exact", 5);
        assert_eq!(result, "exact");
    }

    #[test]
    fn fit_handles_zero_width() {
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn fit_counts_multibyte_characters_as_one_column() {
        let result = fit("café", 6);
        assert_eq!(result.chars().count(), 6);
    }

    #[test]
    fn fit_left_pads_on_the_right_only() {
        let result = fit_left("hi", 6);
        assert_eq!(result, "hi    ");
    }

    #[test]
    fn fit_left_truncates_overlong_text_with_an_ellipsis() {
        let result = fit_left("way too long for this line", 6);
        assert_eq!(result.chars().count(), 6);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn fit_left_handles_zero_width() {
        assert_eq!(fit_left("anything", 0), "");
    }

    // --- belt_row --------------------------------------------------------

    #[test]
    fn belt_row_is_always_exactly_the_requested_width() {
        for width in [0, 1, 7, 72, 200] {
            for tick in 0..8 {
                assert_eq!(belt_row(tick, width).chars().count(), width);
            }
        }
    }

    #[test]
    fn belt_row_only_uses_its_two_glyphs() {
        let row = belt_row(3, 72);
        assert!(row.chars().all(|c| c == '═' || c == '▸'));
    }

    #[test]
    fn belt_row_animates_across_ticks() {
        let first = belt_row(0, 72);
        let later = belt_row(1, 72);
        assert_ne!(first, later, "the belt should visibly move between ticks");
    }

    #[test]
    fn belt_row_repeats_every_pattern_length() {
        assert_eq!(belt_row(0, 40), belt_row(4, 40));
    }

    // --- ids_at ----------------------------------------------------------

    fn card_at(id: &str, station: Station) -> FloorCard {
        FloorCard {
            id: id.to_owned(),
            station,
            landed: false,
        }
    }

    #[test]
    fn an_empty_station_labels_as_nothing() {
        assert_eq!(ids_at(&[], Station::Bench), "");
    }

    #[test]
    fn a_single_occupant_is_named_in_full() {
        let cards = [card_at("F-901", Station::Bench)];
        assert_eq!(ids_at(&cards, Station::Bench), "F-901");
    }

    #[test]
    fn multiple_occupants_name_the_first_and_count_the_rest() {
        // The regression: a comma-join overflowed the cell and truncated to
        // `F-901,F-902…`, which reads as a mangled identifier. Real cycles
        // put more cards at one station than 12 columns can spell out.
        let cards = [
            card_at("F-901", Station::Scan),
            card_at("F-902", Station::Scan),
            card_at("F-903", Station::Scan),
        ];
        assert_eq!(ids_at(&cards, Station::Scan), "F-901 +2");
    }

    #[test]
    fn the_count_only_covers_the_requested_station() {
        let cards = [
            card_at("F-901", Station::Scan),
            card_at("F-902", Station::Bench),
            card_at("F-903", Station::Scan),
        ];
        assert_eq!(ids_at(&cards, Station::Scan), "F-901 +1");
        assert_eq!(ids_at(&cards, Station::Bench), "F-902");
    }

    #[test]
    fn a_full_station_label_always_fits_its_cell() {
        // Six cards at one station labels as `F-901 +5`: 8 columns, inside
        // the 12-column cell, so the ellipsis fallback never triggers for
        // realistic identifiers.
        let cards: Vec<FloorCard> = ["F-901", "F-902", "F-903", "F-904", "F-905", "F-906"]
            .into_iter()
            .map(|id| card_at(id, Station::Weld))
            .collect();
        let label = ids_at(&cards, Station::Weld);
        assert_eq!(label, "F-901 +5");
        assert!(label.chars().count() <= CELL_WIDTH);
    }

    // --- spinner -----------------------------------------------------------

    #[test]
    fn spinner_glyph_cycles_and_wraps() {
        assert_eq!(spinner_glyph(0), spinner_glyph(SPINNER.len()));
        assert_ne!(spinner_glyph(0), spinner_glyph(1));
    }

    // --- frames: structural invariants --------------------------------

    #[test]
    fn full_frames_are_never_empty_and_have_a_fixed_line_count() {
        let sequence = frames(80);
        assert!(!sequence.is_empty());
        for frame in &sequence {
            assert_eq!(frame.len(), FULL_FRAME_LINES);
        }
    }

    #[test]
    fn every_full_frame_line_has_the_same_width_at_its_own_position() {
        let sequence = frames(80);
        let mut expected_widths = [None; FULL_FRAME_LINES];
        for frame in &sequence {
            for (index, line) in frame.iter().enumerate() {
                let width = line.chars().count();
                match expected_widths[index] {
                    None => expected_widths[index] = Some(width),
                    Some(expected) => assert_eq!(
                        expected, width,
                        "line {index} width drifted from {expected} to {width}; \
                         a fixed line count only keeps in-place redraw correct \
                         if every line also holds a fixed width"
                    ),
                }
            }
        }
    }

    #[test]
    fn compact_frames_have_a_fixed_smaller_line_count() {
        let sequence = frames(50);
        assert!(!sequence.is_empty());
        for frame in &sequence {
            assert_eq!(frame.len(), COMPACT_FRAME_LINES);
        }
    }

    #[test]
    fn a_very_narrow_width_does_not_panic_and_stays_fixed_width() {
        let sequence = frames(1);
        assert!(!sequence.is_empty());
        let line_width = sequence[0][0].chars().count();
        for frame in &sequence {
            assert_eq!(frame.len(), COMPACT_FRAME_LINES);
            for line in frame {
                assert_eq!(line.chars().count(), line_width);
            }
        }
    }

    // --- frames: the story actually happens -----------------------------

    fn joined(sequence: &[Frame]) -> String {
        sequence
            .iter()
            .flat_map(|frame| frame.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_card_arrives() {
        let text = joined(&frames(80));
        for card in ["F-901", "F-902", "F-903"] {
            assert!(
                text.contains(&format!("{card} entered the floor")),
                "missing arrival of {card}"
            );
        }
    }

    #[test]
    fn press_reveals_every_sealed_sha() {
        let text = joined(&frames(80));
        for sha in ["7f3a91c", "3c8e04d", "b12aa9e"] {
            assert!(
                text.contains(&format!("▸ {sha} ▪ sealed")),
                "missing seal of {sha}"
            );
        }
    }

    #[test]
    fn one_card_fails_and_is_ejected_then_reworked() {
        let text = joined(&frames(80));
        assert!(text.contains("✗ test-suite failed for F-902 — ejecting"));
        assert!(text.contains("rework"));
        assert!(text.contains("F-902 back on the belt"));
        assert!(text.contains("attempt 2"));
    }

    #[test]
    fn the_three_cards_weld_and_ship_together() {
        let text = joined(&frames(80));
        assert!(text.contains("weld ▸ F-901 + F-902 + F-903"));
        assert!(text.contains("9c1d4e0"));
        assert!(text.contains("authority `main` ▸ 0e77aa1 → 9c1d4e0"));
    }

    #[test]
    fn the_final_frame_reports_every_card_landed_and_the_full_gate_count() {
        let sequence = frames(80);
        let last = sequence.last().expect("non-empty");
        let footer = last.last().expect("footer is the last line");
        assert!(footer.contains("landed today: 3"), "footer: {footer}");
        assert!(footer.contains("4 gates run"), "footer: {footer}");
        assert!(footer.contains("1 reworked"), "footer: {footer}");
        assert!(footer.contains("0 in flight"), "footer: {footer}");
        let ticker = &last[FULL_FRAME_LINES - 2];
        assert!(ticker.contains("demo complete"), "ticker: {ticker}");
    }

    #[test]
    fn frames_is_deterministic() {
        assert_eq!(frames(80), frames(80));
    }

    #[test]
    fn station_commands_names_every_real_command_once() {
        let mapping = station_commands();
        assert_eq!(mapping.len(), 6);
        let commands: Vec<_> = mapping.iter().map(|(_, command)| *command).collect();
        for expected in [
            "work start",
            "gate run",
            "handoff create",
            "review record",
            "integration verify",
            "promote",
        ] {
            assert!(commands.contains(&expected), "missing {expected}");
        }
    }

    // --- play / sinks --------------------------------------------------

    #[test]
    fn play_presents_every_frame_in_order_with_zero_delay() {
        let sequence = frames(80);
        let mut sink = RecordingSink::default();
        play(&sequence, &mut sink, Duration::ZERO);
        assert_eq!(sink.frames, sequence);
    }

    #[test]
    fn null_sink_accepts_frames_and_keeps_no_state() {
        let mut sink = NullSink;
        play(&frames(80), &mut sink, Duration::ZERO);
        // Nothing to assert beyond "did not panic": a sink with no state to
        // read back is exactly the point.
    }

    #[derive(Clone, Default)]
    struct SharedBuffer(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl io::Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).expect("test output is UTF-8")
        }
    }

    #[test]
    fn terminal_sink_hides_the_cursor_and_restores_it_on_drop() {
        let buffer = SharedBuffer::default();
        {
            let mut sink = TerminalSink::new(buffer.clone());
            sink.present(&vec!["hello".to_owned()]);
        }
        let written = buffer.text();
        assert!(
            written.contains("\u{1b}[?25l"),
            "should hide the cursor: {written:?}"
        );
        assert!(written.contains("hello"));
        assert!(
            written.contains("\u{1b}[?25h"),
            "should restore the cursor on drop: {written:?}"
        );
    }

    #[test]
    fn terminal_sink_moves_the_cursor_up_by_the_previous_frame_height() {
        let buffer = SharedBuffer::default();
        let mut sink = TerminalSink::new(buffer.clone());
        sink.present(&vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        sink.present(&vec!["d".to_owned(), "e".to_owned(), "f".to_owned()]);
        let written = buffer.text();
        assert!(
            written.contains("\u{1b}[3A"),
            "should move up 3 lines: {written:?}"
        );
    }

    #[test]
    fn terminal_sink_never_enters_the_alternate_screen_buffer() {
        let buffer = SharedBuffer::default();
        let mut sink = TerminalSink::new(buffer.clone());
        sink.present(&vec!["a".to_owned()]);
        drop(sink);
        let written = buffer.text();
        assert!(
            !written.contains("?1049"),
            "must not touch the alternate screen buffer: {written:?}"
        );
    }
    // --- the beat interpreter beyond the demo's vocabulary ---------------

    fn bare_script(beats: Vec<TimedBeat>) -> Script {
        Script {
            header_base: "cycle T-001 · baseline 0000000".to_owned(),
            header_compact: "T-001 · 0000000".to_owned(),
            progress_total: None,
            beats,
        }
    }

    fn all_text(sequence: &[Frame]) -> String {
        joined(sequence)
    }

    #[test]
    fn a_review_rejection_ejects_the_card_through_the_rework_lane() {
        let script = bare_script(vec![
            TimedBeat::untimed(Beat::Arrive {
                card: "F-800".to_owned(),
            }),
            TimedBeat::untimed(Beat::Advance {
                card: "F-800".to_owned(),
                to: Station::Scan,
            }),
            TimedBeat::untimed(Beat::ReviewReject {
                card: "F-800".to_owned(),
                findings: 3,
            }),
        ]);
        let text = all_text(&frames_for(&script, 80));
        assert!(text.contains("✗ review requested changes on F-800 (3 finding(s)) — ejecting"));
        assert!(text.contains("rework"));
        assert!(text.contains("F-800 back on the belt"));
        let last = frames_for(&script, 80);
        let footer = last.last().unwrap().last().unwrap();
        assert!(footer.contains("1 reworked"), "footer: {footer}");
    }

    #[test]
    fn a_departure_removes_the_card_and_narrates_the_reason() {
        let script = bare_script(vec![
            TimedBeat::untimed(Beat::Arrive {
                card: "F-800".to_owned(),
            }),
            TimedBeat::untimed(Beat::Depart {
                card: "F-800".to_owned(),
                reason: "card abandoned".to_owned(),
            }),
        ]);
        let sequence = frames_for(&script, 80);
        let text = all_text(&sequence);
        assert!(text.contains("F-800 left the floor — card abandoned"));
        let footer = sequence.last().unwrap().last().unwrap();
        assert!(footer.contains("0 in flight"), "footer: {footer}");
    }

    #[test]
    fn notes_and_flashes_narrate_and_flashes_dwell_longer() {
        let noted = bare_script(vec![TimedBeat::untimed(Beat::Note {
            text: "cycle activated".to_owned(),
        })]);
        let flashed = bare_script(vec![TimedBeat::untimed(Beat::Flash {
            text: "✗ evidence: receipt R-1 claims a commit that is gone".to_owned(),
        })]);
        let noted_frames = frames_for(&noted, 80);
        let flashed_frames = frames_for(&flashed, 80);
        assert!(all_text(&noted_frames).contains("cycle activated"));
        assert!(all_text(&flashed_frames).contains("receipt R-1"));
        assert!(
            flashed_frames.len() > noted_frames.len(),
            "a discrepancy must dwell longer than a narration ({} vs {})",
            flashed_frames.len(),
            noted_frames.len()
        );
    }

    #[test]
    fn a_timed_beat_sets_the_history_clock_and_later_untimed_beats_inherit_it() {
        let script = bare_script(vec![
            TimedBeat {
                at: Some("2026-07-24T09:12:00Z".to_owned()),
                progress: None,
                beat: Beat::Note {
                    text: "first".to_owned(),
                },
            },
            TimedBeat::untimed(Beat::Note {
                text: "second".to_owned(),
            }),
        ]);
        let sequence = frames_for(&script, 80);
        let first_header = &sequence[0][0];
        assert!(
            first_header.contains("cycle T-001 · baseline 0000000 · 2026-07-24T09:12:00Z"),
            "header: {first_header}"
        );
        let last_header = &sequence.last().unwrap()[0];
        assert!(
            last_header.contains("2026-07-24T09:12:00Z"),
            "an untimed beat must inherit the clock: {last_header}"
        );
    }

    #[test]
    fn the_progress_counter_appears_only_when_a_total_is_declared() {
        let mut script = bare_script(vec![
            TimedBeat {
                at: None,
                progress: Some(1),
                beat: Beat::Note {
                    text: "first".to_owned(),
                },
            },
            TimedBeat {
                at: None,
                progress: Some(2),
                beat: Beat::Note {
                    text: "second".to_owned(),
                },
            },
        ]);
        let without = frames_for(&script, 80);
        assert!(
            !all_text(&without).contains("event"),
            "no total declared, so no counter"
        );

        script.progress_total = Some(2);
        let with = frames_for(&script, 80);
        let first_footer = with[0].last().unwrap();
        assert!(first_footer.contains("event 1/2"), "footer: {first_footer}");
        let last_footer = with.last().unwrap().last().unwrap();
        assert!(last_footer.contains("event 2/2"), "footer: {last_footer}");
    }

    // --- playback progress ------------------------------------------------

    fn header_percent(frame: &Frame) -> usize {
        let header = &frame[0];
        let start = header.rfind(['█', '░']).map_or_else(
            || panic!("no gauge in header: {header}"),
            |index| index + "█".len(),
        );
        header[start..]
            .trim()
            .trim_end_matches('%')
            .parse()
            .unwrap_or_else(|_| panic!("unparsable percentage in header: {header}"))
    }

    #[test]
    fn every_frame_carries_the_playback_gauge_and_it_never_moves_backwards() {
        let sequence = frames(80);
        let mut previous = 0;
        for frame in &sequence {
            assert!(
                frame[0].contains('%'),
                "the header must carry the gauge: {}",
                frame[0]
            );
            let percent = header_percent(frame);
            assert!(
                percent >= previous,
                "playback progress must never move backwards ({previous} then {percent})"
            );
            previous = percent;
        }
    }

    #[test]
    fn the_final_frame_reads_exactly_one_hundred_percent() {
        let sequence = frames(80);
        assert_eq!(header_percent(sequence.last().unwrap()), 100);
        assert!(
            sequence.last().unwrap()[0].contains("████████ 100%"),
            "a finished bar is completely filled: {}",
            sequence.last().unwrap()[0]
        );
        assert!(
            header_percent(&sequence[0]) < 5,
            "the first of two hundred frames rounds to under five percent"
        );
    }

    #[test]
    fn the_compact_layout_shows_the_percentage_without_a_bar() {
        let sequence = frames(50);
        let last_header = &sequence.last().unwrap()[0];
        assert!(last_header.contains("100%"), "{last_header}");
        assert!(
            !last_header.contains('█'),
            "no room for a bar in compact mode: {last_header}"
        );
    }

    #[test]
    fn the_gauge_shares_the_header_with_the_history_clock() {
        let script = bare_script(vec![
            TimedBeat {
                at: Some("2026-07-24T09:12:00Z".to_owned()),
                progress: None,
                beat: Beat::Note {
                    text: "first".to_owned(),
                },
            },
            TimedBeat::untimed(Beat::Close {
                text: "done".to_owned(),
            }),
        ]);
        let sequence = frames_for(&script, 80);
        let last_header = &sequence.last().unwrap()[0];
        assert!(
            last_header.contains("2026-07-24T09:12:00Z"),
            "the clock survives: {last_header}"
        );
        assert!(
            last_header.contains("100%"),
            "and the gauge rides beside it: {last_header}"
        );
    }

    #[test]
    fn spread_pads_to_exact_width_and_the_right_side_wins_a_collision() {
        let laid = spread("left", "right", 20);
        assert_eq!(laid.chars().count(), 20);
        assert!(laid.starts_with("left"));
        assert!(laid.ends_with("right"));

        let collided = spread(&"x".repeat(30), "100%", 20);
        assert_eq!(collided.chars().count(), 20);
        assert!(
            collided.ends_with("100%"),
            "the gauge must never be the side that truncates: {collided}"
        );
        assert!(
            collided.contains('…'),
            "the left side truncates: {collided}"
        );

        let oversized = spread("left", &"y".repeat(25), 20);
        assert_eq!(oversized.chars().count(), 20);
    }

    #[test]
    fn an_empty_script_renders_no_frames_and_does_not_divide_by_zero() {
        let script = bare_script(Vec::new());
        assert!(frames_for(&script, 80).is_empty());
    }

    #[test]
    fn the_demo_screenplay_never_times_its_beats() {
        let script = demo_script();
        assert!(script.progress_total.is_none());
        assert!(
            script
                .beats
                .iter()
                .all(|beat| beat.at.is_none() && beat.progress.is_none()),
            "the demo header and footer must stay constant"
        );
    }
}
