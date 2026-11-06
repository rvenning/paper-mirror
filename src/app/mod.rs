//! Application shell: startup, window registry and process lifecycle.

pub mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::pdf::engine::{Engine, EngineThread};
use crate::platform_windows::memory;
use window::DocWindow;

/// Command-line options.
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub files: Vec<PathBuf>,
    /// Print startup / first-render timings and memory to stderr.
    pub measure: bool,
    /// Quit once the first page has rendered (used by the performance scripts).
    pub exit_after_render: bool,
    /// Quit after the window is shown and has idled briefly (startup probe).
    pub exit_after_idle: bool,
    pub renderer: Option<String>,
    /// Write a PNG of the window after the first page renders, then quit.
    pub snapshot: Option<PathBuf>,
}

pub fn parse_args(args: &[String]) -> Options {
    let mut o = Options::default();
    for a in args {
        match a.as_str() {
            "--measure" => o.measure = true,
            "--exit-after-render" => o.exit_after_render = true,
            "--exit-after-idle" => o.exit_after_idle = true,
            s if s.starts_with("--renderer=") => o.renderer = Some(s["--renderer=".len()..].to_string()),
            s if s.starts_with("--snapshot=") => {
                o.snapshot = Some(PathBuf::from(&s["--snapshot=".len()..]));
                o.exit_after_render = true;
            }
            s if s.starts_with("--") => {}
            s => o.files.push(PathBuf::from(s)),
        }
    }
    o
}

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<DocWindow>>> = const { RefCell::new(Vec::new()) };
    static CONTEXT: RefCell<Option<AppContext>> = const { RefCell::new(None) };
}

/// Shared services available to every window on the UI thread.
#[derive(Clone)]
pub struct AppContext {
    pub engine: Engine,
    pub options: Options,
    pub started: Instant,
}

pub fn context() -> AppContext {
    CONTEXT.with(|c| c.borrow().clone().expect("application context not initialised"))
}

/// Runs `f` on the window with `id` if it is still open.
pub fn with_window(id: u64, f: impl FnOnce(&Rc<DocWindow>)) {
    let w = WINDOWS.with(|ws| ws.borrow().iter().find(|w| w.id == id).cloned());
    if let Some(w) = w {
        f(&w);
    }
}

pub fn register_window(w: Rc<DocWindow>) {
    WINDOWS.with(|ws| ws.borrow_mut().push(w));
}

pub fn unregister_window(id: u64) {
    let removed = WINDOWS.with(|ws| {
        let mut ws = ws.borrow_mut();
        let pos = ws.iter().position(|w| w.id == id);
        pos.map(|p| ws.remove(p))
    });
    drop(removed);
    if WINDOWS.with(|ws| ws.borrow().is_empty()) {
        let _ = slint::quit_event_loop();
    }
}

pub fn window_count() -> usize {
    WINDOWS.with(|ws| ws.borrow().len())
}

/// Logs a measurement line when `--measure` is on.
pub fn measure(label: &str) {
    let ctx = CONTEXT.with(|c| c.borrow().clone());
    if let Some(ctx) = ctx {
        if ctx.options.measure {
            let m = memory::process_memory();
            eprintln!(
                "[measure] {label}: t={:.1} ms working_set={:.1} MiB private={:.1} MiB peak_ws={:.1} MiB",
                ctx.started.elapsed().as_secs_f64() * 1000.0,
                memory::mib(m.working_set),
                memory::mib(m.private_bytes),
                memory::mib(m.peak_working_set),
            );
        }
    }
}

fn select_backend(renderer: Option<&str>) -> Result<(), String> {
    let renderer = renderer.unwrap_or("software");
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .select()
        .map_err(|e| e.to_string())
}

pub fn run(args: Vec<String>, started: Instant) -> i32 {
    let options = parse_args(&args);
    if let Err(e) = select_backend(options.renderer.as_deref()) {
        eprintln!("paper-mirror: could not initialise the UI backend: {e}");
        return 1;
    }
    let engine_thread: EngineThread = Engine::start();
    CONTEXT.with(|c| {
        *c.borrow_mut() =
            Some(AppContext { engine: engine_thread.engine.clone(), options: options.clone(), started })
    });

    let files = options.files.clone();
    if files.is_empty() {
        if let Err(e) = DocWindow::open_new(None) {
            eprintln!("paper-mirror: {e}");
            return 1;
        }
    } else {
        for f in files {
            if let Err(e) = DocWindow::open_new(Some(f)) {
                eprintln!("paper-mirror: {e}");
                return 1;
            }
        }
    }
    measure("windows-created");
    if options.exit_after_idle {
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), || {
            measure("idle");
            let _ = slint::quit_event_loop();
        });
    }
    slint::Timer::single_shot(std::time::Duration::ZERO, || measure("event-loop-running"));
    let code = match slint::run_event_loop_until_quit() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("paper-mirror: {e}");
            1
        }
    };
    WINDOWS.with(|ws| ws.borrow_mut().clear());
    drop(engine_thread);
    code
}
