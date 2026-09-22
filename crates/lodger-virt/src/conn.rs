//! The libvirt connections, the event loop thread, and the blocking pool.
//!
//! Lodger opens 2 connections to the same URI. The read connection serves
//! lists, events, and consoles. The job connection serves long work such as
//! clone, upload, and snapshot, so a long job never blocks a VM list.
//!
//! Every libvirt call runs in `spawn_blocking`, guarded by a semaphore: 3
//! permits for fast calls and 2 for long jobs. The total of 5 matches
//! libvirtd's default `max_client_requests`. Each operation runs in one
//! closure on one thread, because libvirt stores its last error per thread.

use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use lodger_core::validate::{InputError, check_text};
use tokio::sync::Semaphore;
use virt::connect::Connect;

/// Permits for fast calls on the read connection.
const FAST_PERMITS: usize = 3;
/// Permits for long jobs on the job connection.
const LONG_PERMITS: usize = 2;

/// Why a libvirt operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Input(#[from] InputError),
    #[error("libvirt: {0}")]
    Libvirt(#[from] virt::error::Error),
    #[error("could not start the libvirt event loop: {0}")]
    EventLoop(String),
    /// The closure panicked, or the runtime shut down before it ran.
    #[error("the libvirt call did not finish: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Registers libvirt's default event loop once per process and runs it on
/// its own thread. libvirt needs the loop registered before the first
/// connection opens. Events and keepalive both depend on it.
fn start_event_loop() -> Result<(), Error> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            virt::event::event_register_default_impl().map_err(|e| e.to_string())?;
            thread::Builder::new()
                .name("libvirt-events".into())
                .spawn(|| {
                    loop {
                        if let Err(e) = virt::event::event_run_default_impl() {
                            // One iteration failed. Pause, so a lasting
                            // failure does not spin a CPU core.
                            eprintln!("libvirt event loop: {e}");
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                })
                .map_err(|e| format!("the OS could not start the thread: {e}"))?;
            Ok(())
        })
        .clone()
        .map_err(Error::EventLoop)
}

/// Owns one libvirt connection. Dropping the owner closes the connection:
/// the `Drop` of [`Connect`] calls `virConnectClose`.
///
/// The owner never hands out a clone of the [`Connect`], because each clone
/// holds its own reference and would keep the connection open.
///
/// The `Option` is always `Some` until `drop` takes the [`Connect`] out.
#[derive(Debug)]
struct Connection(Option<Connect>);

impl Connection {
    fn new(conn: Connect) -> Self {
        Self(Some(conn))
    }

    fn get(&self) -> &Connect {
        self.0.as_ref().expect("only drop takes the connection")
    }
}

impl Drop for Connection {
    /// `virConnectClose` can wait on libvirtd, so it must not block a Tokio
    /// worker thread. Inside a runtime, the close runs on the blocking pool.
    /// If the runtime is shutting down and never runs the task, dropping
    /// the task still drops the [`Connect`] and closes it.
    fn drop(&mut self) {
        let Some(conn) = self.0.take() else { return };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(move || drop(conn))),
            Err(_) => drop(conn),
        }
    }
}

/// A shared handle to Lodger's 2 libvirt connections. Clone it freely.
/// When the last clone drops and no call is still running, both
/// connections close.
#[derive(Debug, Clone)]
pub struct Virt {
    read: Arc<Connection>,
    job: Arc<Connection>,
    fast: Arc<Semaphore>,
    long: Arc<Semaphore>,
}

impl Virt {
    /// Starts the event loop if needed, then opens the read and the job
    /// connection to `uri`, for example `qemu:///system`.
    pub async fn open(uri: &str) -> Result<Self, Error> {
        let uri = check_text("libvirt URI", uri)?.to_owned();
        tokio::task::spawn_blocking(move || {
            start_event_loop()?;
            let read = Connect::open(Some(&uri))?;
            let job = Connect::open(Some(&uri))?;
            Ok(Self {
                read: Arc::new(Connection::new(read)),
                job: Arc::new(Connection::new(job)),
                fast: Arc::new(Semaphore::new(FAST_PERMITS)),
                long: Arc::new(Semaphore::new(LONG_PERMITS)),
            })
        })
        .await?
    }

    /// Runs a fast call, such as a list or a lookup, on the read connection.
    pub async fn read<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Connect) -> Result<T, virt::error::Error> + Send + 'static,
        T: Send + 'static,
    {
        run(&self.fast, &self.read, f).await
    }

    /// Runs a long job, such as a clone or an upload, on the job connection.
    pub async fn job<T, F>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Connect) -> Result<T, virt::error::Error> + Send + 'static,
        T: Send + 'static,
    {
        run(&self.long, &self.job, f).await
    }
}

/// Waits for a permit, then runs `f` on a blocking thread with `conn`.
async fn run<T, F>(permits: &Semaphore, conn: &Arc<Connection>, f: F) -> Result<T, Error>
where
    F: FnOnce(&Connect) -> Result<T, virt::error::Error> + Send + 'static,
    T: Send + 'static,
{
    let _permit = permits
        .acquire()
        .await
        .expect("Lodger never closes the semaphores");
    let conn = Arc::clone(conn);
    Ok(tokio::task::spawn_blocking(move || f(conn.get())).await??)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::{Error, LONG_PERMITS, Virt};

    const TEST_URI: &str = "test:///default";

    fn domain_names(conn: &virt::connect::Connect) -> Result<Vec<String>, virt::error::Error> {
        conn.list_all_domains(0)?
            .iter()
            .map(virt::domain::Domain::name)
            .collect()
    }

    #[tokio::test]
    async fn both_connections_list_the_test_driver_domains() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let from_read = virt.read(domain_names).await.unwrap();
        let from_job = virt.job(domain_names).await.unwrap();
        // The test driver always starts with one domain called "test".
        assert_eq!(from_read, ["test"]);
        assert_eq!(from_job, ["test"]);
    }

    #[tokio::test]
    async fn the_connections_are_separate() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        assert!(!Arc::ptr_eq(&virt.read, &virt.job));
        let read_uri = virt.read(|c| c.uri()).await.unwrap();
        let job_uri = virt.job(|c| c.uri()).await.unwrap();
        assert_eq!(read_uri, TEST_URI);
        assert_eq!(job_uri, TEST_URI);
    }

    #[tokio::test]
    async fn opening_twice_starts_the_event_loop_once() {
        let first = Virt::open(TEST_URI).await.unwrap();
        let second = Virt::open(TEST_URI).await.unwrap();
        assert_eq!(first.read(domain_names).await.unwrap(), ["test"]);
        assert_eq!(second.read(domain_names).await.unwrap(), ["test"]);
    }

    #[tokio::test]
    async fn a_uri_with_a_nul_byte_is_rejected() {
        let err = Virt::open("test:///default\0").await.unwrap_err();
        assert!(matches!(err, Error::Input(_)), "{err}");
    }

    #[tokio::test]
    async fn a_libvirt_error_is_returned() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let err = virt
            .read(|c| c.lookup_domain_by_name("lodger-spike-missing"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Libvirt(_)), "{err}");
    }

    #[tokio::test]
    async fn a_panic_in_a_call_is_returned_as_an_error() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let err = virt
            .read(|_| -> Result<(), _> { panic!("test panic") })
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Task(_)), "{err}");
        // The permit came back, and the connection still works.
        assert_eq!(virt.read(domain_names).await.unwrap(), ["test"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_jobs_do_not_delay_a_fast_read() {
        const JOB_TIME: Duration = Duration::from_secs(2);
        let virt = Virt::open(TEST_URI).await.unwrap();

        // Fill every long permit, and queue one more job behind them.
        let jobs: Vec<_> = (0..=LONG_PERMITS)
            .map(|_| {
                let virt = virt.clone();
                tokio::spawn(async move {
                    virt.job(|c| {
                        std::thread::sleep(JOB_TIME);
                        c.uri()
                    })
                    .await
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let start = Instant::now();
        assert_eq!(virt.read(domain_names).await.unwrap(), ["test"]);
        let read_time = start.elapsed();
        assert!(read_time < JOB_TIME / 4, "the read took {read_time:?}");

        // The queued job waited for a permit, so the jobs took two rounds.
        let start = Instant::now();
        for job in jobs {
            job.await.unwrap().unwrap();
        }
        assert!(start.elapsed() > JOB_TIME + JOB_TIME / 2);
    }

    #[tokio::test]
    async fn dropping_the_last_handle_closes_both_connections() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let clone = virt.clone();
        let read = Arc::downgrade(&virt.read);
        let job = Arc::downgrade(&virt.job);

        drop(virt);
        assert!(read.upgrade().is_some(), "a clone still holds the handle");
        drop(clone);
        // No owner is left, so each `Connect` dropped and closed.
        assert!(read.upgrade().is_none());
        assert!(job.upgrade().is_none());
    }

    #[tokio::test]
    async fn the_last_handle_can_drop_outside_a_runtime() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let read = Arc::downgrade(&virt.read);
        // A plain OS thread has no Tokio runtime, so the close runs inline.
        std::thread::spawn(move || drop(virt)).join().unwrap();
        assert!(read.upgrade().is_none());
    }
}
