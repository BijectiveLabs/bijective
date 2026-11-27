// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A run that is still going: stdin stays open and output is read as it arrives.
//! `docs/concepts/strategies.md` carries why the two halves cannot share a reader.

use std::io::{BufRead, BufReader, Write};
use std::process::Child;
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, channel, sync_channel,
};
use std::time::{Duration, Instant};

use crate::Exit;
use crate::cgroup::RunCgroup;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub lines: Vec<String>,
    /// Distinct from having said nothing this moment: a supervisor that could not tell would feed a dead process forever.
    pub closed: bool,
}

/// Not `Completed`: output arrives line by line, so an empty `stdout` would read as a strategy that said nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub exit: Exit,
    pub stderr: Vec<u8>,
    pub elapsed: std::time::Duration,
}

/// Measured, not chosen: 1,024 is what the platform absorbed. A default, not a policy: the right slack is an operator decision.
pub const QUEUED_BY_DEFAULT: usize = 1024;

/// Long enough for a program to notice end of input and leave, short enough not to be waited on.
const GOODBYE: Duration = Duration::from_millis(500);

/// Two states because the remedies differ: a closed input means finished, a full one means still there and not listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsent {
    Behind,
    Gone,
}

impl core::fmt::Display for Unsent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unsent::Behind => write!(
                f,
                "it has stopped taking its events, and its queue and its input are both full"
            ),
            Unsent::Gone => write!(f, "its input is closed"),
        }
    }
}

impl std::error::Error for Unsent {}

pub struct Live {
    child: Child,
    queued: Option<SyncSender<Vec<u8>>>,
    writing: Option<std::thread::JoinHandle<()>>,
    lines: Receiver<Result<String, String>>,
    complaining: Option<std::thread::JoinHandle<Vec<u8>>>,
    started: Instant,
    closed: bool,
}

impl Live {
    /// Same spawn as the bounded lifetime, read as it speaks.
    /// `holding` is passed in: one number in the source cannot serve operators whose tolerances differ.
    pub fn start(
        cgroup: &RunCgroup,
        layout: &crate::RunLayout,
        holding: usize,
    ) -> std::io::Result<Self> {
        Ok(Self::over(crate::spawn(cgroup, layout)?, holding))
    }

    /// The reader is a thread: this crate forbids `unsafe`, and a blocking read on the calling thread would stop the feed while a strategy is quiet.
    pub fn over(mut child: Child, holding: usize) -> Self {
        // Written on a thread as the bounded half is: a live session has no wall bound to end a supervisor blocked on a full pipe.
        let mut queued = None;
        let mut writing = None;
        if let Some(mut pipe) = child.stdin.take() {
            // At least one, because a queue of nothing is a session that ends on its first event.
            let (events, waiting) = sync_channel::<Vec<u8>>(holding.max(1));
            queued = Some(events);
            writing = Some(std::thread::spawn(move || {
                for event in waiting {
                    // A failed write ends the thread and disconnects the queue, so the next `send` reports it.
                    if pipe.write_all(&event).and_then(|()| pipe.flush()).is_err() {
                        break;
                    }
                }
            }));
        }

        let (tx, lines) = channel();

        if let Some(out) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines() {
                    // Sent, not logged and dropped: a swallowed read error would leave a strategy looking merely quiet.
                    let message = line.map_err(|e| e.to_string());
                    let failed = message.is_err();
                    if tx.send(message).is_err() || failed {
                        break;
                    }
                }
            });
        }

        // Read too: a strategy that logs more than a pipe holds blocks, and stops reading its events.
        let complaining = crate::collect(child.stderr.take());

        Self { child, queued, writing, lines, complaining, started: Instant::now(), closed: false }
    }

    /// Queued, not written here: a write to a full pipe would block the supervisor with no wall bound to rescue it. The queue fills instead and says so.
    /// A broken pipe is reported one event late: the failure is the writer's.
    pub fn send(&mut self, line: &[u8]) -> Result<(), Unsent> {
        let Some(queue) = self.queued.as_ref() else {
            return Err(Unsent::Gone);
        };
        match queue.try_send(line.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(Unsent::Behind),
            Err(TrySendError::Disconnected(_)) => Err(Unsent::Gone),
        }
    }

    pub fn hear(&mut self) -> Result<Heard, std::io::Error> {
        let mut lines = Vec::new();
        loop {
            match self.lines.try_recv() {
                Ok(Ok(line)) => lines.push(line),
                Ok(Err(e)) => return Err(std::io::Error::other(e)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    break;
                }
            }
        }
        Ok(Heard { lines, closed: self.closed })
    }

    /// Waiting form of `hear`: asking and sleeping between asks costs the sleep on every event.
    /// `hear` stays: a live session must not stop reading the market because a strategy is quiet.
    pub fn wait(&mut self, patience: Duration) -> Result<Heard, std::io::Error> {
        let first = match self.lines.recv_timeout(patience) {
            Ok(Ok(line)) => line,
            Ok(Err(e)) => return Err(std::io::Error::other(e)),
            Err(RecvTimeoutError::Timeout) => {
                return Ok(Heard { lines: Vec::new(), closed: self.closed });
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                return Ok(Heard { lines: Vec::new(), closed: true });
            }
        };

        let mut heard = self.hear()?;
        heard.lines.insert(0, first);
        Ok(heard)
    }

    /// The whole process group: `unshare --fork` puts the work in a grandchild that would otherwise outlive the direct child holding the pipes.
    pub fn stop(mut self, cgroup: &RunCgroup) -> std::io::Result<Ended> {
        // The input closing is how a session ends. Ending the group first would record every session as cut short.
        // The grace period is explicit: `end_group`'s own latency used to be the only thing letting a strategy exit first.
        drop(self.queued.take());
        let deadline = Instant::now() + GOODBYE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Out of grace, or unaskable. Ending the group also releases a writer blocked on a stalled pipe.
                _ => {
                    crate::end_group(self.child.id());
                    let _ = self.child.kill();
                    break;
                }
            }
        }
        if let Some(writing) = self.writing.take() {
            let _ = writing.join();
        }

        let status = self.child.wait()?;
        let elapsed = self.started.elapsed();
        let stderr = self.complaining.take().and_then(|h| h.join().ok()).unwrap_or_default();

        // Read from the cgroup, not inferred from a signal: a memory kill and our own kill exit alike.
        let exit = if cgroup.was_killed().unwrap_or(false) {
            Exit::LimitExceeded
        } else {
            Exit::Completed { status: status.code().unwrap_or(-1) }
        };

        Ok(Ended { exit, stderr, elapsed })
    }
}
