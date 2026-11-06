//! Application shell: startup, shared services, window registry and process lifecycle.

pub mod window;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::pdf::engine::{Engine, EngineThread};
use crate::platform_windows::memory;
use crate::storage::settings::{self, Settings};
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
    /// Diagnostics: search for this text once the first document opens.
    pub search: Option<String>,
    /// Diagnostics: commands to run once the first document opens.
    pub commands: Vec<String>,
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
            s if s.starts_with("--search=") => o.search = Some(s["--search=".len()..].to_string()),
            s if s.starts_with("--command=") => o.commands.push(s["--command=".len()..].to_string()),
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
    pub settings: Rc<RefCell<Settings>>,
    pub settings_path: PathBuf,
    /// True when running on the winit backend (false under the test backend).
    pub winit: bool,
}

pub fn context() -> AppContext {
    CONTEXT.with(|c| c.borrow().clone().expect("application context not initialised"))
}

/// Installs the shared context. Called by `run` and by UI tests.
pub fn install_context(ctx: AppContext) {
    CONTEXT.with(|c| *c.borrow_mut() = Some(ctx));
    apply_appearance();
}

/// Mutates settings and persists them immediately (atomic write; failures are
/// reported once to stderr and otherwise ignored, since preferences are not critical).
pub fn update_settings(f: impl FnOnce(&mut Settings)) {
    let ctx = context();
    f(&mut ctx.settings.borrow_mut());
    if let Err(e) = ctx.settings.borrow().save_to(&ctx.settings_path) {
        eprintln!("paper-mirror: could not save settings: {e}");
    }
}

/// Runs `f` on the window with `id` if it is still open.
pub fn with_window(id: u64, f: impl FnOnce(&Rc<DocWindow>)) {
    let w = WINDOWS.with(|ws| ws.borrow().iter().find(|w| w.id == id).cloned());
    if let Some(w) = w {
        f(&w);
    }
}

pub fn windows() -> Vec<Rc<DocWindow>> {
    WINDOWS.with(|ws| ws.borrow().clone())
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
    if let Some(w) = &removed {
        w.release_document();
    }
    drop(removed);
    if WINDOWS.with(|ws| ws.borrow().is_empty()) {
        let _ = slint::quit_event_loop();
    } else {
        refresh_window_menus();
    }
}

/// The window already showing `path`, if any (Preview focuses it instead of
/// opening a second copy).
pub fn find_window_for(path: &Path) -> Option<Rc<DocWindow>> {
    let key = settings::normalise(path);
    windows().into_iter().find(|w| w.doc_path().is_some_and(|p| settings::normalise(&p) == key))
}

pub fn activate_window_at(index: usize) {
    if let Some(w) = windows().get(index) {
        w.ui.window().set_minimized(false);
        let _ = w.ui.show();
    }
}

/// Pushes the recent-files list and the open-window list into every window's menus.
pub fn refresh_window_menus() {
    let ctx = context();
    let recent: Vec<SharedString> =
        ctx.settings.borrow().recent.iter().map(|p| SharedString::from(p.display().to_string())).collect();
    let all = windows();
    let titles: Vec<SharedString> = all.iter().map(|w| SharedString::from(w.title())).collect();
    for (i, w) in all.iter().enumerate() {
        w.ui.set_recent_files(ModelRc::new(VecModel::from(recent.clone())));
        w.ui.set_window_titles(ModelRc::new(VecModel::from(titles.clone())));
        w.ui.set_window_index(i as i32);
    }
}

/// Applies the appearance preference. `Theme` is a global per window.
pub fn apply_appearance() {
    let appearance = context().settings.borrow().appearance.clone();
    for w in windows() {
        w.ui.global::<crate::ui::Theme>().set_appearance(appearance.clone().into());
    }
}

/// Logs a measurement line when `--measure` is on.
pub fn measure(label: &str) {
    let ctx = CONTEXT.with(|c| c.borrow().clone());
    if let Some(ctx) = ctx
        && ctx.options.measure
    {
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
    let settings_path = Settings::path();
    let settings = Settings::load_from(&settings_path);
    install_context(AppContext {
        engine: engine_thread.engine.clone(),
        options: options.clone(),
        started,
        settings: Rc::new(RefCell::new(settings)),
        settings_path,
        winit: true,
    });

    let files = options.files.clone();
    let result = if files.is_empty() {
        DocWindow::open_new(None).map(|_| ())
    } else {
        files.into_iter().try_for_each(|f| DocWindow::open_new(Some(f)).map(|_| ()))
    };
    if let Err(e) = result {
        eprintln!("paper-mirror: {e}");
        return 1;
    }
    apply_appearance();
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
