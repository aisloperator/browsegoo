//! Debugging hangs, as in issue #40: logs when the browser's main loop, or a
//! tab's web process, stops responding, and captures what the processes are
//! doing then. Only on with the environment variable `BROWSEGOO_DEBUG_STALLS`
//! set to 1, and otherwise nothing here runs.
//!
//! The log goes to standard error. When the main loop or a web process has
//! been stuck for `BROWSEGOO_DEBUG_HANG_MS` milliseconds (1000 by default),
//! the hang is captured to `browsegoo-debug-hangs-<timestamp>.txt` in the
//! current directory: each thread's CPU use, and the backtraces (with
//! `eu-stack` if it's installed, or else `gdb`) and executable memory maps of
//! BrowseGoo and of the processes that are busy.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use gtk::glib;
use webkit::prelude::*;

static START: OnceLock<Instant> = OnceLock::new();
/// Milliseconds since `START` when the main loop last ran the heartbeat.
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
/// For each tab, when the script pings it hasn't answered yet were sent.
static PINGS: Mutex<Vec<(u64, VecDeque<u64>)>> = Mutex::new(Vec::new());

/// Whether `BROWSEGOO_DEBUG_STALLS` is 1.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("BROWSEGOO_DEBUG_STALLS").is_some_and(|value| value == "1")
    })
}

fn now() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn log(message: &str) {
    if enabled() {
        eprintln!("[stall {:>9.3}] {message}", now() as f64 / 1000.0);
    }
}

/// Starts the main loop's heartbeat, and a thread that says when it's late.
pub fn start() {
    if !enabled() {
        return;
    }
    now();
    let main_tid = unsafe { libc_gettid() };
    glib::timeout_add_local(Duration::from_millis(50), || {
        HEARTBEAT.store(now(), Ordering::Relaxed);
        glib::ControlFlow::Continue
    });
    thread::Builder::new()
        .name("stall-watch".into())
        .spawn(move || {
            let mut stalled_since = None;
            let mut capture = capture::Capturer::new();
            loop {
                thread::sleep(Duration::from_millis(50));
                let late = now().saturating_sub(HEARTBEAT.load(Ordering::Relaxed));
                capture.check(late, oldest_unanswered_ping());
                if late > 300 {
                    if stalled_since.is_none() {
                        stalled_since = Some(now() - late);
                        let syscall =
                            std::fs::read_to_string(format!("/proc/self/task/{main_tid}/syscall"))
                                .unwrap_or_default();
                        let state =
                            std::fs::read_to_string(format!("/proc/self/task/{main_tid}/stat"))
                                .unwrap_or_default();
                        let state = state.rsplit(')').next().unwrap_or("").trim();
                        log(&format!(
                            "UI main loop STALLED (tid {main_tid}, syscall {}, stat {})",
                            syscall.trim(),
                            state.split(' ').take(1).collect::<Vec<_>>().join(" ")
                        ));
                    }
                } else if let Some(since) = stalled_since.take() {
                    log(&format!("UI main loop resumed after {} ms", now() - since));
                }
            }
        })
        .expect("the stall watcher should start");
}

unsafe extern "C" {
    fn gettid() -> i32;
}

unsafe fn libc_gettid() -> i32 {
    unsafe { gettid() }
}

fn pings() -> std::sync::MutexGuard<'static, Vec<(u64, VecDeque<u64>)>> {
    PINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How long the oldest unanswered script ping has waited, and for which tab.
fn oldest_unanswered_ping() -> Option<(u64, u64)> {
    let now = now();
    pings()
        .iter()
        .filter_map(|(tab, sent_times)| Some((*tab, now - *sent_times.front()?)))
        .max_by_key(|(_, waited)| *waited)
}

/// Logs a tab's web process going unresponsive, and pings it with a script
/// to time how long its main thread takes to answer.
pub fn watch(id: u64, webview: &webkit::WebView) {
    if !enabled() {
        return;
    }
    webview.connect_is_web_process_responsive_notify(move |webview| {
        log(&format!(
            "tab {id}: web process responsive = {}",
            webview.is_web_process_responsive()
        ));
    });
    webview.connect_load_changed(move |webview, event| {
        log(&format!(
            "tab {id}: load {event:?} {}",
            webview.uri().unwrap_or_default()
        ));
    });
    let weak = webview.downgrade();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(webview) = weak.upgrade() else {
            pings().retain(|(tab, _)| *tab != id);
            return glib::ControlFlow::Break;
        };
        let sent = now();
        {
            let mut pings = pings();
            match pings.iter_mut().find(|(tab, _)| *tab == id) {
                Some((_, sent_times)) => sent_times.push_back(sent),
                None => pings.push((id, VecDeque::from([sent]))),
            }
        }
        webview.evaluate_javascript("1", None, None, None::<&gtk::gio::Cancellable>, move |_| {
            if let Some((_, sent_times)) = pings().iter_mut().find(|(tab, _)| *tab == id) {
                sent_times.retain(|time| *time != sent);
            }
            let took = now() - sent;
            if took > 250 {
                log(&format!("tab {id}: script ping took {took} ms"));
            }
        });
        glib::ControlFlow::Continue
    });
}

/// Logs, each second, the scroll events reaching `window`: how many, how many
/// came again with the same time (as WebKit puts back events a page didn't
/// use), and the longest history and largest delta among them.
pub fn watch_scrolling(window: &gtk::ApplicationWindow) {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    use gtk::gdk;
    use gtk::prelude::*;

    if !enabled() {
        return;
    }
    #[derive(Default)]
    struct Counts {
        events: u32,
        again: u32,
        history: usize,
        delta: f64,
        seen: HashSet<u32>,
    }
    let counts = Rc::new(RefCell::new(Counts::default()));
    let legacy = gtk::EventControllerLegacy::new();
    legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
    let counting = counts.clone();
    legacy.connect_event(move |_, event| {
        if let Some(scroll) = event.downcast_ref::<gdk::ScrollEvent>() {
            let mut counts = counting.borrow_mut();
            counts.events += 1;
            if !counts.seen.insert(scroll.time()) {
                counts.again += 1;
            }
            counts.history = counts.history.max(scroll.history().len());
            let (dx, dy) = scroll.deltas();
            counts.delta = counts.delta.max(dx.abs().max(dy.abs()));
        }
        glib::Propagation::Proceed
    });
    window.add_controller(legacy);
    glib::timeout_add_local(Duration::from_secs(1), move || {
        let mut counts = counts.borrow_mut();
        if counts.events > 0 {
            log(&format!(
                "scroll: {} events, {} again, longest history {}, largest delta {:.0}",
                counts.events, counts.again, counts.history, counts.delta
            ));
        }
        let seen = std::mem::take(&mut counts.seen);
        *counts = Counts::default();
        // Keep the times seen, to notice events coming back a second later.
        counts.seen = seen;
        if counts.seen.len() > 100_000 {
            counts.seen.clear();
        }
        glib::ControlFlow::Continue
    });
}

mod capture {
    //! Captures what the processes are doing during a hang.

    use std::fs;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    use gtk::glib;

    use super::{HashMap, log, now};

    /// The most captures in one run, so a long hang doesn't fill the disk.
    const MOST: u32 = 30;
    /// The least time between captures.
    const APART_MS: u64 = 8000;
    /// How long threads' CPU use is measured for.
    const SAMPLE: Duration = Duration::from_millis(300);

    pub struct Capturer {
        threshold: u64,
        count: u32,
        last: Option<u64>,
        running: Option<thread::JoinHandle<()>>,
    }

    impl Capturer {
        pub fn new() -> Self {
            let threshold = std::env::var("BROWSEGOO_DEBUG_HANG_MS")
                .ok()
                .and_then(|text| text.parse().ok())
                .unwrap_or(1000);
            Self {
                threshold,
                count: 0,
                last: None,
                running: None,
            }
        }

        /// Starts a capture if the main loop is `late` or a tab's ping has
        /// waited too long, and one isn't running or just done.
        pub fn check(&mut self, late: u64, ping: Option<(u64, u64)>) {
            if self
                .running
                .as_ref()
                .is_some_and(|running| !running.is_finished())
            {
                return;
            }
            self.running = None;
            if self.count >= MOST
                || self
                    .last
                    .is_some_and(|last| now().saturating_sub(last) < APART_MS)
            {
                return;
            }
            let reason = if late >= self.threshold {
                format!("UI main loop {late} ms late")
            } else if let Some((tab, waited)) = ping.filter(|(_, waited)| *waited >= self.threshold)
            {
                format!("tab {tab}'s script ping unanswered for {waited} ms")
            } else {
                return;
            };
            self.count += 1;
            self.last = Some(now());
            let timestamp = glib::DateTime::now_local()
                .and_then(|time| time.format("%Y%m%d-%H%M%S"))
                .map_or_else(|_| format!("{}", now()), |text| text.to_string());
            let path = format!("browsegoo-debug-hangs-{timestamp}.txt");
            log(&format!("HANG: {reason}; capturing to {path}"));
            self.running = thread::Builder::new()
                .name("hang-capture".into())
                .spawn(move || {
                    let started = now();
                    let text = capture(&reason);
                    match fs::File::create(&path)
                        .and_then(|mut file| file.write_all(text.as_bytes()))
                    {
                        Ok(()) => log(&format!("captured {path} in {} ms", now() - started)),
                        Err(error) => log(&format!("could not write {path}: {error}")),
                    }
                })
                .ok();
        }
    }

    /// BrowseGoo and its WebKit processes: their IDs and names.
    fn processes() -> Vec<(u32, String)> {
        let own = std::process::id();
        let mut found = vec![(own, "browsegoo".to_owned())];
        let Ok(entries) = fs::read_dir("/proc") else {
            return found;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            let comm = comm.trim();
            if comm.starts_with("WebKit") && pid != own {
                found.push((pid, comm.to_owned()));
            }
        }
        found
    }

    /// Each thread's CPU time so far, in clock ticks, with its name.
    fn thread_times(pid: u32) -> HashMap<u32, (u64, String)> {
        let mut times = HashMap::new();
        let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
            return times;
        };
        for task in tasks.flatten() {
            let Some(tid) = task
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")) else {
                continue;
            };
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else {
                continue;
            };
            let name = stat[open + 1..close].to_owned();
            // After the name: state, then 10 more fields, then utime and stime.
            let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
            let ticks = |index: usize| {
                fields
                    .get(index)
                    .and_then(|field| field.parse::<u64>().ok())
            };
            if let (Some(user), Some(system)) = (ticks(11), ticks(12)) {
                times.insert(tid, (user + system, format!("{name} [{}]", fields[0])));
            }
        }
        times
    }

    fn capture(reason: &str) -> String {
        let mut out = format!(
            "Hang: {reason}\nAt {:.3} s after start\n\n",
            now() as f64 / 1000.0
        );
        let processes = processes();

        // Which threads are using the CPU now.
        let before: Vec<_> = processes
            .iter()
            .map(|(pid, _)| thread_times(*pid))
            .collect();
        thread::sleep(SAMPLE);
        let ticks_per_second = 100.0;
        let mut busy_processes = Vec::new();
        let mut rows = Vec::new();
        for ((pid, comm), before) in processes.iter().zip(&before) {
            let mut process_percent = 0.0;
            for (tid, (ticks, name)) in thread_times(*pid) {
                let earlier = before.get(&tid).map_or(ticks, |(earlier, _)| *earlier);
                let percent =
                    (ticks - earlier) as f64 / ticks_per_second / SAMPLE.as_secs_f64() * 100.0;
                process_percent += percent;
                if percent > 0.0 {
                    rows.push((percent, format!("{pid:>8} {comm:<16} {tid:>8} {name}")));
                }
            }
            if process_percent >= 20.0 {
                busy_processes.push(*pid);
            }
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.push_str(&format!(
            "Threads using CPU over {} ms:\n",
            SAMPLE.as_millis()
        ));
        for (percent, row) in rows.iter().take(25) {
            out.push_str(&format!("{percent:>5.0}% {row}\n"));
        }
        out.push_str("\nProcesses:\n");
        for (pid, comm) in &processes {
            out.push_str(&format!("{pid:>8} {comm}\n"));
        }

        // Backtraces of BrowseGoo itself, and of the busy ones, all at once.
        let own = std::process::id();
        let mut dump: Vec<u32> = vec![own];
        dump.extend(busy_processes.iter().filter(|pid| **pid != own));
        if dump.len() == 1 {
            // Nothing busy: perhaps something is waiting, so take every web process.
            dump.extend(
                processes
                    .iter()
                    .filter(|(_, comm)| comm.starts_with("WebKitWebProces"))
                    .map(|(pid, _)| *pid),
            );
        }
        let use_eu_stack = Command::new("eu-stack")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        let children: Vec<_> = dump
            .iter()
            .map(|pid| {
                let mut command = if use_eu_stack {
                    let mut command = Command::new("eu-stack");
                    command.args(["-p", &pid.to_string()]);
                    command
                } else {
                    // WebKit's processes are sandboxed in their own mount
                    // namespace, where gdb can't read their files, so it's
                    // given the same files from outside it.
                    let executable = fs::read_link(format!("/proc/{pid}/exe")).unwrap_or_default();
                    let mut command = Command::new("gdb");
                    command
                        .args([
                            "-nx",
                            "-batch",
                            "-iex",
                            "set sysroot /",
                            "-iex",
                            "set debuginfod enabled off",
                            "-ex",
                            "thread apply all bt",
                        ])
                        .arg(executable)
                        .args(["-p", &pid.to_string()]);
                    command
                };
                let child = command
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
                (*pid, child)
            })
            .collect();
        for (pid, child) in children {
            let comm = processes
                .iter()
                .find(|(other, _)| *other == pid)
                .map_or("", |(_, comm)| comm.as_str());
            out.push_str(&format!("\n===== {pid} {comm}\n"));
            match child.and_then(|child| child.wait_with_output()) {
                Ok(output) => {
                    out.push_str(&String::from_utf8_lossy(&output.stdout));
                    out.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                Err(error) => out.push_str(&format!("Could not get a backtrace: {error}\n")),
            }
            out.push_str(&format!("\n----- maps of {pid}\n"));
            let maps = fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default();
            // Only the executable mappings are needed to name functions.
            for line in maps.lines().filter(|line| {
                line.split_whitespace()
                    .nth(1)
                    .is_some_and(|perms| perms.contains('x'))
            }) {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}
