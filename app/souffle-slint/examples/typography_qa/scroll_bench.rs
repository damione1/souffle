//! Scripted scrolling of a fixture list with one scroll step per rendered frame.
//!
//! SOU276_SCROLL_BENCH=1 runs three passes (slow, fast, jumps), prints one
//! line per pass and quits. The list moves once every step interval
//! (SOU276_SCROLL_BENCH_INTERVAL_MS, 16 by default; 8 is one step per frame
//! of a 120 Hz display). Each step is charged the main-thread CPU time spent
//! until the next one (change handlers, row creation, layout and the draw),
//! and its wall time: the timer runs on that same thread, so a step that
//! overruns delays the next one.
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

unsafe extern "C" {
    fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
}

/// Darwin's CLOCK_THREAD_CPUTIME_ID.
const THREAD_CPU_CLOCK: i32 = 16;

fn thread_cpu_ms() -> f64 {
    let mut time = Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    unsafe { clock_gettime(THREAD_CPU_CLOCK, &mut time) };
    time.seconds as f64 * 1e3 + time.nanoseconds as f64 / 1e6
}

/// How the bench reads and moves a list, in px scrolled down from the top.
pub(super) struct View<W> {
    pub(super) travel: Reading<W>,
    pub(super) position: Reading<W>,
    pub(super) scroll_to: ScrollTo<W>,
}

pub(super) type Reading<W> = Box<dyn Fn(&W) -> f32>;
pub(super) type ScrollTo<W> = Box<dyn Fn(&W, f32)>;

impl<W: ComponentHandle + 'static> View<W> {
    /// A list scrolled like a user does, by wheel events at (`x`, `y`). The
    /// window does not report that list's position, so it is tracked here
    /// against the `travel` the caller computed.
    pub(super) fn wheel(x: f32, y: f32, travel: f32) -> Self {
        let scrolled = Rc::new(std::cell::Cell::new(0.0f32));
        let position = scrolled.clone();
        Self {
            travel: Box::new(move |_| travel),
            position: Box::new(move |_| position.get()),
            scroll_to: Box::new(move |window, target| {
                let delta = target - scrolled.replace(target);
                window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                        position: slint::LogicalPosition::new(x, y),
                        delta_x: 0.0,
                        delta_y: -delta,
                    });
            }),
        }
    }
}

pub(super) fn enabled() -> bool {
    std::env::var("SOU276_SCROLL_BENCH").as_deref() == Ok("1")
}

/// One step of a pass. A wheel moves the list by a distance from where it
/// is; a scroll bar drag puts it at a share of its travel. The two differ for
/// a list that estimates its height: its offsets shift as the estimate does.
#[derive(Clone, Copy)]
enum Move {
    By(f32),
    ToShare(f32),
}

struct Pass {
    name: &'static str,
    moves: Vec<Move>,
}

#[derive(Default)]
struct Samples {
    cpu: Vec<f64>,
    wall: Vec<f64>,
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// 60 steps a second unless SOU276_SCROLL_BENCH_INTERVAL_MS says otherwise.
fn step_interval() -> std::time::Duration {
    std::time::Duration::from_millis(
        std::env::var("SOU276_SCROLL_BENCH_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(16),
    )
}

/// A step this late has dropped at least one 60 Hz frame.
const STALL_MS: f64 = 33.4;
/// The frame time of a 120 Hz display, and of a 60 Hz one.
const FRAME_120_MS: f64 = 1000.0 / 120.0;
const FRAME_60_MS: f64 = 1000.0 / 60.0;

fn report(name: &str, label: &str, samples: &Samples) {
    let mut cpu = samples.cpu.clone();
    let mut wall = samples.wall.clone();
    cpu.sort_by(f64::total_cmp);
    wall.sort_by(f64::total_cmp);
    let stalls = wall.iter().filter(|ms| **ms > STALL_MS).count();
    let over = |budget: f64| cpu.iter().filter(|ms| **ms > budget).count();
    eprintln!(
        "bench {label} {name}: steps {} cpu_ms mean {:.2} p50 {:.2} p95 {:.2} max {:.2} over_8.3ms {} over_16.7ms {} stalls {} wall_ms p50 {:.2} p95 {:.2} max {:.2}",
        cpu.len(),
        cpu.iter().sum::<f64>() / cpu.len().max(1) as f64,
        percentile(&cpu, 0.5),
        percentile(&cpu, 0.95),
        cpu.last().copied().unwrap_or(0.0),
        over(FRAME_120_MS),
        over(FRAME_60_MS),
        stalls,
        percentile(&wall, 0.5),
        percentile(&wall, 0.95),
        wall.last().copied().unwrap_or(0.0),
    );
}

fn passes(frames: usize) -> Vec<Pass> {
    // A scroll bar drag: far jumps, each followed by two small moves.
    let mut seed = 0x2545_f491u32;
    let jumps = (0..frames / 3)
        .flat_map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let share = (seed >> 8) as f32 / (1u32 << 24) as f32;
            [Move::ToShare(share), Move::By(20.0), Move::By(20.0)]
        })
        .collect();
    vec![
        Pass {
            name: "slow_12px",
            moves: vec![Move::By(12.0); frames],
        },
        Pass {
            name: "fast_150px",
            moves: vec![Move::By(150.0); frames],
        },
        Pass {
            name: "jumps",
            moves: jumps,
        },
    ]
}

struct Run {
    passes: Vec<Pass>,
    pass: usize,
    frame: usize,
    // Flipped at either end, so a pass longer than the list goes back and forth.
    direction: f32,
    samples: Samples,
    started: Option<(Instant, f64)>,
}

/// Scrolls the list `view` describes. `label` is printed with each pass;
/// `finished` runs before quitting.
pub(super) fn start<W: ComponentHandle + 'static>(
    window: &W,
    label: String,
    view: View<W>,
    finished: impl Fn(&W) + 'static,
) {
    let frames = std::env::var("SOU276_SCROLL_BENCH_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600);
    let run: RefCell<Option<Run>> = RefCell::new(None);
    let weak = window.as_weak();
    let timer = Rc::new(slint::Timer::default());
    let ticking = timer.clone();
    let tick = move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let mut slot = run.borrow_mut();
        let run = slot.get_or_insert_with(|| {
            eprintln!("bench {label}: travel {:.0} px", (view.travel)(&window));
            Run {
                passes: passes(frames),
                pass: 0,
                frame: 0,
                direction: 1.0,
                samples: Samples::default(),
                started: None,
            }
        });
        let now = (Instant::now(), thread_cpu_ms());
        if let Some((wall, cpu)) = run.started.replace(now) {
            run.samples.cpu.push(now.1 - cpu);
            run.samples.wall.push((now.0 - wall).as_secs_f64() * 1e3);
        }
        if run.frame >= run.passes[run.pass].moves.len() {
            report(run.passes[run.pass].name, &label, &run.samples);
            run.samples = Samples::default();
            run.frame = 0;
            run.direction = 1.0;
            run.pass += 1;
            if run.pass >= run.passes.len() {
                eprintln!("bench {label}: travel {:.0} px", (view.travel)(&window));
                ticking.stop();
                drop(slot);
                finished(&window);
                slint::quit_event_loop().unwrap();
                return;
            }
        }
        let travel = (view.travel)(&window);
        let position = match run.passes[run.pass].moves[run.frame] {
            Move::By(distance) => {
                let from = (view.position)(&window);
                if from >= travel {
                    run.direction = -1.0;
                } else if from <= 0.0 {
                    run.direction = 1.0;
                }
                from + run.direction * distance
            }
            Move::ToShare(share) => share * travel,
        };
        run.frame += 1;
        drop(slot);
        (view.scroll_to)(&window, position.clamp(0.0, travel));
    };
    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
        if weak.upgrade().is_some() {
            timer.start(slint::TimerMode::Repeated, step_interval(), tick);
        }
    });
}

/// Times `steps` calls of `step`, one every step interval, for a view that
/// grows instead of scrolling. Reports the first and the last quarter apart:
/// the cost of a step should not follow the amount of content.
pub(super) fn start_growth<W: ComponentHandle + 'static>(
    window: &W,
    label: String,
    steps: usize,
    step: impl Fn(&W, usize) + 'static,
) {
    let weak = window.as_weak();
    let timer = Rc::new(slint::Timer::default());
    let ticking = timer.clone();
    #[derive(Default)]
    struct Growth {
        done: usize,
        samples: Samples,
        started: Option<(Instant, f64)>,
    }
    let state = RefCell::new(Growth::default());
    let tick = move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let mut state = state.borrow_mut();
        let now = (Instant::now(), thread_cpu_ms());
        if let Some((wall, cpu)) = state.started.replace(now) {
            state.samples.cpu.push(now.1 - cpu);
            state.samples.wall.push((now.0 - wall).as_secs_f64() * 1e3);
        }
        if state.done >= steps {
            let quarter = (steps / 4).max(1);
            let part = |range: std::ops::Range<usize>| Samples {
                cpu: state.samples.cpu[range.clone()].to_vec(),
                wall: state.samples.wall[range].to_vec(),
            };
            report("first_quarter", &label, &part(0..quarter));
            report("last_quarter", &label, &part(steps - quarter..steps));
            ticking.stop();
            slint::quit_event_loop().unwrap();
            return;
        }
        let index = state.done;
        state.done += 1;
        drop(state);
        step(&window, index);
    };
    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
        if weak.upgrade().is_some() {
            timer.start(slint::TimerMode::Repeated, step_interval(), tick);
        }
    });
}
