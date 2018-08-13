// #![deny(missing_docs)]
use futures::{Future, Poll};

use std::error::Error;
use std::{fmt, io};
use std::time::{Duration, Instant};

use tokio_connect::Connect;
use tokio::timer::{self, Deadline, DeadlineError};
use tokio::io::{AsyncRead, AsyncWrite};
use tower_service::Service;

/// A timeout that wraps an underlying operation.
#[derive(Debug, Clone)]
pub struct Timeout<'name, T> {
    inner: T,
    duration: Duration,
    name: &'name str,
}


/// An error representing that an operation timed out.
#[derive(Debug)]
pub struct TimeoutError<'name, E> {
    name: &'name str,
    kind: TimeoutErrorKind<E>,
}

#[derive(Debug)]
enum TimeoutErrorKind<E> {
    /// Indicates the underlying operation timed out.
    Timeout(Duration),
    /// Indicates that the underlying operation failed.
    Error(E),
    // Indicates that the timer returned an error.
    Timer(timer::Error),
}

/// A duration which pretty-prints as fractional seconds.
///
/// This may not be the ideal display format for _all_ duration values,
/// but should be sufficient for most timeouts.
#[derive(Copy, Clone, Debug)]
pub struct HumanDuration(pub Duration);

//===== impl Timeout =====

impl<T> Timeout<'static, T> {
    /// Construct a new `Timeout` wrapping `inner`.
    pub fn new(inner: T, duration: Duration) -> Self {
        Timeout {
            inner,
            duration,
            name: "operation",
        }
    }
}

impl<'name, T> Timeout<'name, T> {
    pub fn named<'a>(self, name: &'a str) -> Timeout<'a, T> {
        Timeout {
            inner: self.inner,
            duration: self.duration,
            name,
        }
    }

    fn error<E>(&self, error: E) -> TimeoutError<'name, E> {
        TimeoutError {
            name: self.name,
            kind: TimeoutErrorKind::Error(error),
        }
    }

    fn deadline_error<E>(&self, error: DeadlineError<E>) -> TimeoutError<'name, E> {
        let kind = match error {
            _ if error.is_timer() =>
                TimeoutErrorKind::Timer(error.into_timer()
                    .expect("error.into_timer() must succeed if error.is_timer()")),
            _ if error.is_elapsed() =>
                TimeoutErrorKind::Timeout(self.duration),
            _ => TimeoutErrorKind::Error(error.into_inner()
                .expect("if error is not elapsed or timer, must be inner")),
        };
        TimeoutError { name: self.name, kind }
    }
}

impl<'name, S, T, E> Service for Timeout<'name, S>
where
    S: Service<Response=T, Error=E>,
{
    type Request = S::Request;
    type Response = T;
    type Error = TimeoutError<'name, E>;
    type Future = Timeout<'name, Deadline<S::Future>>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready().map_err(|e| self.error(e))
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let duration = self.duration;
        let deadline = Instant::now() + duration;
        let inner = Deadline::new(self.inner.call(req), deadline);
        Timeout {
            inner,
            duration: self.duration,
            name: self.name,
        }
    }
}


impl<'name, C> Connect for Timeout<'name, C>
where
    C: Connect,
{
    type Connected = C::Connected;
    type Error = TimeoutError<'name, C::Error>;
    type Future = Timeout<'name, Deadline<C::Future>>;

    fn connect(&self) -> Self::Future {
        let deadline = Instant::now() + self.duration;
        let inner = Deadline::new(self.inner.connect(), deadline);
        Timeout {
            inner,
            duration: self.duration,
            name: self.name
        }
    }
}

impl<'name, F> Future for Timeout<'name, Deadline<F>>
where
    F: Future,
    // F::Error: Error,
{
    type Item = F::Item;
    type Error = TimeoutError<'name, F::Error>;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll().map_err(|e| self.deadline_error(e))
    }
}


impl<'name, C> io::Read for Timeout<'name, C>
where
    C: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<'name, C> io::Write for Timeout<'name, C>
where
    C: io::Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<'name, C> AsyncRead for Timeout<'name, C>
where
    C: AsyncRead,
{
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        self.inner.prepare_uninitialized_buffer(buf)
    }
}

impl<'name, C> AsyncWrite for Timeout<'name, C>
where
    C: AsyncWrite,
{
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        self.inner.shutdown()
    }
}

//===== impl TimeoutError =====

impl<'name, E> fmt::Display for TimeoutError<'name, E>
where
    E: fmt::Display
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            TimeoutErrorKind::Timeout(ref d) =>
                write!(f, "{} timed out after {}", self.name, HumanDuration(*d)),
            TimeoutErrorKind::Timer(ref err) => write!(f, "timer failed: {}", err),
            TimeoutErrorKind::Error(ref err) => fmt::Display::fmt(err, f),
        }
    }
}

impl<'name, E> Error for TimeoutError<'name, E>
where
    E: Error
{
    fn cause(&self) -> Option<&Error> {
        match self.kind {
            TimeoutErrorKind::Error(ref err) => Some(err),
            TimeoutErrorKind::Timer(ref err) => Some(err),
            _ => None,
        }
    }
}

//===== impl HumanDuration =====

impl fmt::Display for HumanDuration {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let secs = self.0.as_secs();
        let subsec_ms = self.0.subsec_nanos() as f64 / 1_000_000f64;
        if secs == 0 {
            write!(fmt, "{}ms", subsec_ms)
        } else {
            write!(fmt, "{}s", secs as f64 + subsec_ms)
        }
    }
}

impl From<Duration> for HumanDuration {

    #[inline]
    fn from(d: Duration) -> Self {
        HumanDuration(d)
    }
}
