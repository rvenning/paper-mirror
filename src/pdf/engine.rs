//! The PDF engine thread. PDFium is not thread safe, so every PDFium call made by the
//! application runs here. The UI thread submits closures with a priority and an
//! optional generation token; jobs whose token went stale while queued (because the
//! user scrolled or zoomed on) are dropped without running.

use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use super::document::PdfDoc;

pub type DocId = u64;

/// State owned by the engine thread.
#[derive(Default)]
pub struct EngineState {
    pub docs: HashMap<DocId, PdfDoc>,
}

impl EngineState {
    pub fn doc(&self, id: DocId) -> Option<&PdfDoc> {
        self.docs.get(&id)
    }

    pub fn doc_mut(&mut self, id: DocId) -> Option<&mut PdfDoc> {
        self.docs.get_mut(&id)
    }
}

/// Higher runs first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Background = 0,
    Thumbnail = 1,
    Render = 2,
    Interactive = 3,
}

/// A cancellation token: the job runs only if the counter still holds `value`.
#[derive(Clone)]
pub struct Generation {
    counter: Arc<AtomicU64>,
    value: u64,
}

impl Generation {
    pub fn is_current(&self) -> bool {
        self.counter.load(Ordering::Acquire) == self.value
    }
}

/// A counter the UI bumps to invalidate every job issued under the previous value.
#[derive(Clone, Default)]
pub struct GenerationCounter(Arc<AtomicU64>);

impl GenerationCounter {
    pub fn bump(&self) -> Generation {
        let value = self.0.fetch_add(1, Ordering::AcqRel) + 1;
        Generation { counter: self.0.clone(), value }
    }

    pub fn current(&self) -> Generation {
        Generation { counter: self.0.clone(), value: self.0.load(Ordering::Acquire) }
    }
}

type JobFn = Box<dyn FnOnce(&mut EngineState) + Send>;

struct Job {
    priority: Priority,
    seq: u64,
    generation: Option<Generation>,
    f: JobFn,
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl Eq for Job {}
impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Job {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Highest priority first; FIFO within a priority (lower seq first).
        self.priority.cmp(&other.priority).then_with(|| other.seq.cmp(&self.seq))
    }
}

enum Msg {
    Job(Job),
    Shutdown,
}

#[derive(Clone)]
pub struct Engine {
    tx: Sender<Msg>,
    seq: Arc<AtomicU64>,
    next_doc: Arc<AtomicU64>,
}

pub struct EngineThread {
    pub engine: Engine,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start() -> EngineThread {
        let (tx, rx) = channel();
        let handle = std::thread::Builder::new()
            .name("pdf-engine".into())
            .spawn(move || run(rx))
            .expect("failed to start the PDF engine thread");
        EngineThread {
            engine: Engine { tx, seq: Arc::new(AtomicU64::new(0)), next_doc: Arc::new(AtomicU64::new(1)) },
            handle: Some(handle),
        }
    }

    pub fn new_doc_id(&self) -> DocId {
        self.next_doc.fetch_add(1, Ordering::Relaxed)
    }

    pub fn submit(
        &self,
        priority: Priority,
        generation: Option<Generation>,
        f: impl FnOnce(&mut EngineState) + Send + 'static,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(Msg::Job(Job { priority, seq, generation, f: Box::new(f) }));
    }

    /// Runs a job and blocks for its result. For tests and shutdown paths only; the UI
    /// thread must never call this.
    pub fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut EngineState) -> R + Send + 'static) -> R {
        let (tx, rx) = channel();
        self.submit(Priority::Interactive, None, move |s| {
            let _ = tx.send(f(s));
        });
        rx.recv().expect("PDF engine thread stopped")
    }
}

impl Drop for EngineThread {
    fn drop(&mut self) {
        let _ = self.engine.tx.send(Msg::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(rx: Receiver<Msg>) {
    let mut state = EngineState::default();
    let mut queue: BinaryHeap<Job> = BinaryHeap::new();
    loop {
        // Block only when there is nothing queued, then drain everything pending so
        // priorities are respected across the whole backlog.
        if queue.is_empty() {
            match rx.recv() {
                Ok(Msg::Job(j)) => queue.push(j),
                Ok(Msg::Shutdown) | Err(_) => break,
            }
        }
        let mut shutdown = false;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Job(j) => queue.push(j),
                Msg::Shutdown => shutdown = true,
            }
        }
        if shutdown {
            break;
        }
        if let Some(job) = queue.pop()
            && job.generation.as_ref().is_none_or(Generation::is_current)
        {
            // A panic inside a job (e.g. a PDFium edge case surfacing as a Rust
            // bounds error) must not take down the engine thread.
            let f = job.f;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut state)));
            if r.is_err() {
                eprintln!("paper-mirror: a PDF engine job panicked; continuing");
            }
        }
    }
    // Close documents before the thread exits.
    state.docs.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn priorities_and_cancellation() {
        let t = Engine::start();
        let e = t.engine.clone();
        let log = Arc::new(Mutex::new(Vec::new()));
        // Hold the engine busy so all following jobs queue up together.
        let (gate_tx, gate_rx) = channel::<()>();
        e.submit(Priority::Interactive, None, move |_| {
            let _ = gate_rx.recv();
        });
        let counter = GenerationCounter::default();
        let stale = counter.bump();
        for (p, name) in [(Priority::Thumbnail, "thumb"), (Priority::Render, "render"), (Priority::Background, "bg")] {
            let log = log.clone();
            e.submit(p, None, move |_| log.lock().unwrap().push(name));
        }
        {
            let log = log.clone();
            e.submit(Priority::Interactive, Some(stale), move |_| log.lock().unwrap().push("stale"));
        }
        let _fresh = counter.bump();
        gate_tx.send(()).unwrap();
        e.call(|_| ());
        // `call` runs at Interactive, so drain the lower priorities explicitly.
        let (done_tx, done_rx) = channel();
        e.submit(Priority::Background, None, move |_| done_tx.send(()).unwrap());
        done_rx.recv().unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["render", "thumb", "bg"]);
    }

    #[test]
    fn panicking_job_does_not_kill_engine() {
        let t = Engine::start();
        t.engine.submit(Priority::Interactive, None, |_| panic!("boom"));
        assert_eq!(t.engine.call(|_| 42), 42);
    }
}
