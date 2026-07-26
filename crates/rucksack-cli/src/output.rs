use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Everything rucksack says to the person running it.
///
/// Four verbs, one line each, no blank lines: `pack` is a two-second command and its whole output
/// should fit in a glance. `detail` is the escape hatch for the probes and paths behind a step,
/// and only `--verbose` shows it.
#[derive(Debug, Clone)]
pub struct Output {
    verbose: bool,
    color: bool,
    live: bool,
}

impl Output {
    pub fn new(verbose: bool) -> Self {
        let terminal = io::stdout().is_terminal();
        Self {
            verbose,
            color: terminal && std::env::var_os("NO_COLOR").is_none(),
            // Deliberately not folded into `color`: NO_COLOR asks for no colour, not for no motion,
            // and a live row is not decoration — it is the only thing distinguishing "waiting" from
            // "hung". What it does depend on is a terminal, because the alternative consumer of this
            // output is an agent capturing stdout, and a carriage return means nothing to a pipe.
            live: terminal,
        }
    }

    /// What just happened.
    pub fn step(&self, message: impl AsRef<str>) {
        println!("{}", message.as_ref());
    }

    /// The verdict the reader is waiting for. Always the last line.
    pub fn done(&self, message: impl AsRef<str>) {
        println!("{}", self.paint(BOLD, message.as_ref()));
    }

    /// Something the user should know, which did not stop rucksack.
    pub fn warn(&self, message: impl AsRef<str>) {
        println!("{} {}", self.paint(YELLOW, "!"), message.as_ref());
    }

    pub fn detail(&self, message: impl AsRef<str>) {
        if self.verbose {
            println!("  {}", message.as_ref());
        }
    }

    /// Run `work`, saying what is being waited for until it finishes.
    ///
    /// The two readers of rucksack's output want opposite things here, and both are real. A person at
    /// a terminal wants one row whose clock moves, because a still screen is indistinguishable from a
    /// hang. An agent capturing stdout wants whole lines it can relay, and must never receive a
    /// stream of carriage returns. So the terminal gets one row repainted every second, the pipe gets
    /// a line on a widening schedule, and both are rendered by `frame` so their words cannot drift
    /// apart.
    ///
    /// `label` is `&'static str` on purpose: nothing measured or user-supplied may widen the row, since
    /// a carriage return only returns to the start of the *last* terminal line and a wrapped row would
    /// strand everything above it.
    pub fn waiting<T>(&self, label: &'static str, work: impl FnOnce() -> T) -> T {
        let done = Arc::new(AtomicBool::new(false));
        let painter = self.spawn_painter(label, Arc::clone(&done));
        let outcome = work();
        done.store(true, Ordering::Release);
        if let Some(painter) = painter {
            // The thread wakes at most REDRAW late, and clearing is its job, not ours.
            let _ = painter.join();
        }
        outcome
    }

    fn spawn_painter(
        &self,
        label: &'static str,
        done: Arc<AtomicBool>,
    ) -> Option<thread::JoinHandle<()>> {
        let live = self.live;
        Some(thread::spawn(move || {
            let started = Instant::now();
            let mut next = FIRST_TICK;
            let mut painted = 0usize;
            while !done.load(Ordering::Acquire) {
                let elapsed = started.elapsed();
                if live {
                    // Nothing at all for the first moment, so a fast operation stays silent.
                    if elapsed >= GRACE {
                        paint_over(&frame(label, elapsed), &mut painted);
                    }
                } else if elapsed >= next {
                    println!("{}", frame(label, elapsed));
                    next = next_tick(next);
                }
                thread::sleep(if live { REDRAW } else { POLL });
            }
            if live && painted > 0 {
                erase(painted);
            }
        }))
    }

    fn paint(&self, code: &str, value: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{value}\x1b[0m")
        } else {
            value.to_owned()
        }
    }
}

/// What a waiting row says. Pure, and the single source of both consumers' words.
fn frame(label: &str, elapsed: Duration) -> String {
    format!("{label} {}", clock(elapsed))
}

/// Elapsed time as a person reads it, truncated so it never claims time that has not passed.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// When a captured stream gets its next line.
///
/// Doubling, with growth capped, because the marginal value of a repeated line falls away: an agent
/// reading `2:00` already knows nothing changed since `1:00`. A four-minute wait costs four lines
/// instead of eight, and each one carries a number the last did not.
fn next_tick(previous: Duration) -> Duration {
    (previous * 2).min(previous + MAX_GAP)
}

/// Overwrite the current row, remembering how wide it was.
///
/// Erasure is spaces and a carriage return rather than an escape sequence, which keeps the carriage
/// return the only control byte rucksack ever writes. That is sound only because a row never gets
/// narrower — `a_waiting_row_never_narrows` is what keeps it true.
fn paint_over(text: &str, painted: &mut usize) {
    let mut stdout = io::stdout().lock();
    let _ = write!(stdout, "\r{text}");
    let _ = stdout.flush();
    *painted = (*painted).max(text.chars().count());
}

fn erase(painted: usize) {
    let mut stdout = io::stdout().lock();
    let _ = write!(stdout, "\r{}\r", " ".repeat(painted));
    let _ = stdout.flush();
}

const BOLD: &str = "1";
const YELLOW: &str = "33";

/// How long an operation may take before it is worth mentioning at all.
const GRACE: Duration = Duration::from_millis(500);
/// How often a terminal row is repainted.
const REDRAW: Duration = Duration::from_secs(1);
/// How often the painter checks whether the work finished, when there is nothing to repaint.
const POLL: Duration = Duration::from_millis(100);
/// When a captured stream gets its first line. Matches the tick a piped `pack` printed before this
/// existed, so an agent's view of a short wait is unchanged.
const FIRST_TICK: Duration = Duration::from_secs(30);
/// The most a captured stream will ever go between lines.
const MAX_GAP: Duration = Duration::from_secs(120);

/// What rucksack waits for, as fixed strings so a row can never be widened by a measured value.
pub const WAITING_NETWORK: &str = "Still waiting for the network — lid open.";
pub const WAITING_IPHONE: &str = "Still waiting for the iPhone — lid open.";
pub const WAITING_JOIN: &str = "Joining…";
pub const WAITING_WATCHER: &str = "Starting the safety watcher…";

/// Render a failure as "what stopped" then "what to do about it".
///
/// Messages carry their own next action on a second line, so the only formatting here is the
/// marker and a two-space indent that keeps the action visually attached to its cause.
pub fn render_error(error: &anyhow::Error) -> String {
    let mut rendered = String::new();
    for (index, line) in format!("{error:#}").lines().enumerate() {
        rendered.push_str(if index == 0 { "× " } else { "  " });
        rendered.push_str(line);
        rendered.push('\n');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_leads_with_the_cause_and_indents_the_next_action() {
        let error = anyhow::anyhow!("The battery is at 12%.\nPlug in, then try again.");

        assert_eq!(
            render_error(&error),
            "× The battery is at 12%.\n  Plug in, then try again.\n"
        );
    }

    /// A waiting row says what it waits for and how long it has waited, and nothing else.
    #[test]
    fn a_waiting_row_says_what_it_waits_for_and_for_how_long() {
        assert_eq!(
            frame(WAITING_NETWORK, Duration::from_millis(1_900)),
            "Still waiting for the network — lid open. 0:01"
        );
        assert_eq!(clock(Duration::ZERO), "0:00");
        assert_eq!(clock(Duration::from_secs(59)), "0:59");
        assert_eq!(clock(Duration::from_secs(60)), "1:00");
        assert_eq!(clock(Duration::from_secs(247)), "4:07");
        assert_eq!(clock(Duration::from_secs(6_000)), "100:00");
    }

    /// Truncated, never rounded up: at 1.9s only one second has actually passed.
    #[test]
    fn a_clock_never_claims_time_that_has_not_passed() {
        for millis in [999u64, 1_000, 1_999] {
            let shown = clock(Duration::from_millis(millis));
            let claimed: u64 = shown.split(':').next_back().unwrap().parse().unwrap();
            assert!(
                claimed * 1_000 <= millis,
                "{shown} claims more than {millis}ms"
            );
        }
    }

    /// The invariant that makes erasing with spaces sound: a row never gets narrower, so the spaces
    /// written to clear it always cover everything that was there.
    #[test]
    fn a_waiting_row_never_narrows() {
        for label in [
            WAITING_NETWORK,
            WAITING_IPHONE,
            WAITING_JOIN,
            WAITING_WATCHER,
        ] {
            let mut widest = 0;
            // Six hours, which is well past the 24-hour ceiling's worst realistic wait.
            for second in 0..(6 * 60 * 60) {
                let width = frame(label, Duration::from_secs(second)).chars().count();
                assert!(width >= widest, "{label} narrowed at {second}s");
                widest = width;
            }
        }
    }

    /// A captured stream pays fewer lines for a longer wait, not more.
    ///
    /// The first line still lands at thirty seconds, so an agent's view of a short wait is exactly
    /// what it was before any of this existed.
    #[test]
    fn a_long_wait_costs_fewer_lines_not_more() {
        let mut at = FIRST_TICK;
        let mut ticks = vec![at];
        while at < Duration::from_secs(240) {
            at = next_tick(at);
            ticks.push(at);
        }
        assert_eq!(
            ticks,
            vec![
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(120),
                Duration::from_secs(240),
            ],
            "a four-minute wait should cost four lines, where it used to cost eight"
        );

        // And the gap stops widening, so a very long wait still says something occasionally.
        assert_eq!(
            next_tick(Duration::from_secs(240)),
            Duration::from_secs(360)
        );
        assert_eq!(
            next_tick(Duration::from_secs(600)),
            Duration::from_secs(720)
        );
    }

    /// The bounded waits are all shorter than the first captured line, so wrapping them cannot add a
    /// line to what an agent reads. Naming the durations here means changing one fails this test
    /// rather than silently growing pack's output.
    #[test]
    fn a_bounded_wait_never_reaches_a_captured_stream() {
        for bounded in [
            Duration::from_secs(20), // AUTOMATIC_JOIN_TIMEOUT
            Duration::from_secs(8),  // the watcher handshake
        ] {
            assert!(
                bounded < FIRST_TICK,
                "{bounded:?} would print a line to a pipe"
            );
            assert!(
                bounded > GRACE,
                "{bounded:?} is too short to be worth saying"
            );
        }
    }
}
