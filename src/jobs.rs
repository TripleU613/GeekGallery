//! Upload progress, tracked per job so the page can show what is happening.
//!
//! An upload takes the best part of a minute, nearly all of it Gemini generating
//! an image. A form that just hangs for that long looks broken, so the POST
//! returns a job id straight away and the browser polls this registry for the
//! step it is on.
//!
//! In memory, deliberately. There is exactly one app process and "no storage on
//! the server" is a hard rule, so a table or a Redis would both be wrong here.
//! The cost is that a restart loses in-flight jobs: the browser sees the id go
//! missing and says so, rather than waiting forever on a job nobody is running.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use crate::models::Progress;

/// Kept long enough that a tab left in the background still finds its result,
/// short enough that nothing accumulates. Swept on write, not by a timer task.
const RETAIN: Duration = Duration::from_secs(300);

struct Entry {
    progress: Progress,
    touched: Instant,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static R: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many jobs may be inside the pipeline at once. A job holds its whole
/// file in memory -- up to 60MB for a clip, plus the decoded frames of a
/// still -- inside a container capped at 1GB, so an unbounded number of them
/// is an OOM kill, and an OOM kill loses every job in flight. Past this many,
/// a new job waits its turn; the browser sees it "receiving" a little longer.
const CONCURRENT: usize = 3;

fn slots() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static S: OnceLock<std::sync::Arc<tokio::sync::Semaphore>> = OnceLock::new();
    S.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENT)))
}

/// Run a job's pipeline off the request, one of `CONCURRENT` at a time, with
/// its outcome guaranteed: a pipeline that panics (a decoder tripping on a
/// crafted file) is recorded as failed instead of left "running" until the
/// registry forgets it, which is what the browser would otherwise poll for
/// five minutes.
pub fn spawn<F>(job: String, pipeline: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        // `acquire_owned` only errors once the semaphore is closed, which
        // nothing does; treat that as a free pass rather than a stuck job.
        let permit = slots().clone().acquire_owned().await.ok();
        let outcome = tokio::spawn(pipeline).await;
        drop(permit);
        let Err(e) = outcome else { return };
        if !e.is_panic() {
            return;
        }
        tracing::error!("job {job} panicked: {e}");
        set(
            &job,
            Progress::Failed {
                message: "something went wrong processing that file".to_string(),
            },
        );
    });
}

/// Register a new job and return its id.
pub fn start() -> String {
    let id = uuid::Uuid::new_v4().to_string();
    set(
        &id,
        Progress::Running {
            step: crate::models::Step::Receiving,
        },
    );
    id
}

/// Record where a job has got to. Sweeps anything stale while it holds the lock,
/// which is cheap and means no background task exists just to tidy up.
pub fn set(id: &str, progress: Progress) {
    // A poisoned lock means a previous holder panicked mid-update. Progress
    // reporting must never be the reason an upload fails, so recover the guard
    // and carry on rather than propagating the panic.
    let mut map = match registry().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.retain(|_, e| e.touched.elapsed() < RETAIN);
    map.insert(
        id.to_string(),
        Entry {
            progress,
            touched: Instant::now(),
        },
    );
}

/// `None` once a job has expired or if the id was never real. The caller turns
/// that into a `Failed`, since from the browser's side they are the same thing:
/// there is nothing more coming.
pub fn get(id: &str) -> Option<Progress> {
    let map = match registry().lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.get(id).map(|e| e.progress.clone())
}
