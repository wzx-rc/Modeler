//! Runs the kernel process (`forme-nurbs-kernel`) for the app.
//!
//! One worker thread talks to the process: it sends the latest document,
//! waits for the model, then sends the latest document again if it changed
//! meanwhile (the latest request wins, older ones are never sent). A hard
//! crash inside OpenCascade kills the process, not the app: the thread
//! starts it again with the step it was on (last `STEP` line on stderr)
//! skipped, and the model lists it.

use crate::doc::Doc;
use crate::model::Model;
use crate::proto::{read_frame, write_frame, Request};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The kernel process: `FORME_NURBS_KERNEL` if set, else
/// `forme-nurbs-kernel` next to the running executable (or one folder up).
pub fn kernel_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FORME_NURBS_KERNEL") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("forme-nurbs-kernel{}", std::env::consts::EXE_SUFFIX);
    [dir.join(&name), dir.parent()?.join(&name)].into_iter().find(|p| p.is_file())
}

/// A finished build.
pub struct Built {
    pub model: Model,
    /// Request to answer, in the app's time (ms).
    pub round_ms: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No kernel process found.
    Missing,
    Ready,
    /// It crashed and could not be restarted.
    Broken(String),
}

struct Job {
    serial: u64,
    fine: bool,
    doc: Doc,
}

#[derive(Default)]
struct Inner {
    pending: Option<Job>,
    done: Option<Built>,
    busy: bool,
    quit: bool,
    status: Option<Status>,
}

struct Shared {
    m: Mutex<Inner>,
    cv: Condvar,
}

pub struct Kernel {
    shared: Arc<Shared>,
    exe: Option<PathBuf>,
}

impl Kernel {
    /// Starts the worker thread (the process starts with the first request).
    pub fn start(exe: Option<PathBuf>) -> Kernel {
        let shared = Arc::new(Shared { m: Mutex::new(Inner::default()), cv: Condvar::new() });
        shared.m.lock().unwrap().status = Some(if exe.is_some() { Status::Ready } else { Status::Missing });
        if let Some(exe) = exe.clone() {
            let s = shared.clone();
            std::thread::Builder::new().name("nurbs-kernel".into()).spawn(move || worker(s, exe)).ok();
        }
        Kernel { shared, exe }
    }

    pub fn exe(&self) -> Option<&PathBuf> {
        self.exe.as_ref()
    }

    /// Asks for this document (replaces a request not sent yet).
    pub fn submit(&self, serial: u64, doc: &Doc, fine: bool) {
        let mut g = self.shared.m.lock().unwrap();
        g.pending = Some(Job { serial, fine, doc: doc.clone() });
        self.shared.cv.notify_all();
    }

    /// The latest finished build, once.
    pub fn take(&self) -> Option<Built> {
        self.shared.m.lock().unwrap().done.take()
    }

    /// A request is waiting or being built.
    pub fn busy(&self) -> bool {
        let g = self.shared.m.lock().unwrap();
        g.busy || g.pending.is_some()
    }

    pub fn status(&self) -> Status {
        self.shared.m.lock().unwrap().status.clone().unwrap_or(Status::Missing)
    }

    /// Waits for the build of request `serial` (or a later one), at most
    /// `timeout` (headless captures and tests).
    pub fn wait_for(&self, serial: u64, timeout: Duration) -> Option<Built> {
        let end = Instant::now() + timeout;
        let mut g = self.shared.m.lock().unwrap();
        loop {
            if g.done.as_ref().map(|b| b.model.serial >= serial).unwrap_or(false) {
                return g.done.take();
            }
            if g.status.as_ref().map(|s| *s == Status::Missing).unwrap_or(false) {
                return None;
            }
            let now = Instant::now();
            if now >= end {
                return None;
            }
            g = self.shared.cv.wait_timeout(g, end - now).unwrap().0;
        }
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        let mut g = self.shared.m.lock().unwrap();
        g.quit = true;
        self.shared.cv.notify_all();
    }
}

struct Proc {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: ChildStdout,
    last_step: Arc<Mutex<Option<String>>>,
    err_thread: Option<std::thread::JoinHandle<()>>,
}

impl Proc {
    fn spawn(exe: &PathBuf) -> std::io::Result<Proc> {
        let mut cmd = Command::new(exe);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let input = BufWriter::new(child.stdin.take().unwrap());
        let output = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let last_step = Arc::new(Mutex::new(None));
        let ls = last_step.clone();
        let err_thread = std::thread::Builder::new()
            .name("nurbs-kernel-stderr".into())
            .spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    if let Some(s) = line.strip_prefix("STEP ") {
                        *ls.lock().unwrap() = Some(s.trim().to_string());
                    }
                }
            })
            .ok();
        Ok(Proc { child, input, output, last_step, err_thread })
    }

    /// The step it was on when it died.
    fn bury(mut self) -> Option<String> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.input);
        if let Some(t) = self.err_thread.take() {
            let _ = t.join();
        }
        self.last_step.lock().unwrap().take()
    }
}

/// How many steps may be skipped in a session before giving up.
const MAX_SKIPS: usize = 24;

fn worker(shared: Arc<Shared>, exe: PathBuf) {
    let mut proc: Option<Proc> = None;
    let mut skip: Vec<String> = vec![];
    loop {
        let job = {
            let mut g = shared.m.lock().unwrap();
            while g.pending.is_none() && !g.quit {
                g = shared.cv.wait(g).unwrap();
            }
            if g.quit {
                break;
            }
            g.busy = true;
            g.pending.take().unwrap()
        };
        let t0 = Instant::now();
        let mut result: Option<Model> = None;
        let mut failure = String::new();
        for _attempt in 0..4 {
            if proc.is_none() {
                match Proc::spawn(&exe) {
                    Ok(p) => proc = Some(p),
                    Err(e) => {
                        failure = format!("cannot start the kernel ({e})");
                        break;
                    }
                }
            }
            let p = proc.as_mut().unwrap();
            *p.last_step.lock().unwrap() = None;
            let req = Request { serial: job.serial, fine: job.fine, skip: skip.clone(), doc: job.doc.clone() };
            let bytes = serde_json::to_vec(&req).unwrap_or_default();
            let answer = write_frame(&mut p.input, &bytes).ok().and_then(|_| read_frame(&mut p.output)).and_then(|b| Model::decode(&b));
            match answer {
                Some(m) => {
                    result = Some(m);
                    break;
                }
                None => {
                    // it died: skip the step it was on and try again
                    let step = proc.take().unwrap().bury();
                    match step {
                        Some(s) if !s.starts_with("mesh:") && !skip.contains(&s) && skip.len() < MAX_SKIPS => skip.push(s),
                        Some(s) => {
                            failure = format!("the kernel crashed ({s})");
                            break;
                        }
                        None => {
                            failure = "the kernel crashed".to_string();
                            break;
                        }
                    }
                }
            }
        }
        let mut g = shared.m.lock().unwrap();
        g.busy = false;
        let model = match result {
            Some(m) => {
                g.status = Some(Status::Ready);
                m
            }
            None => {
                g.status = Some(Status::Broken(failure.clone()));
                Model { serial: job.serial, fine: job.fine, errors: vec![failure], ..Default::default() }
            }
        };
        // a newer request may be waiting: this answer still shows (the app
        // keeps the newest serial it has seen)
        g.done = Some(Built { model, round_ms: t0.elapsed().as_secs_f32() * 1000.0 });
        shared.cv.notify_all();
    }
    if let Some(p) = proc {
        let _ = p.bury();
    }
}
