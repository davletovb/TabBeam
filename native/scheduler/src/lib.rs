//! Fair, bounded scheduling for provider exchanges.
//!
//! The scheduler owns provider exchanges and their lifecycle limits. It has no
//! browser or Native Messaging concepts. The supervisor is the panic boundary
//! shared by synchronous and service-thread entry points.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use runtime_core::exchange::{Exchange, Timeouts, Update};
use runtime_core::protocol::Failure;

const STOP_SLACK: Duration = Duration::from_secs(1);

pub type TurnId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    Start,
    Idle,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Completed,
    Failed(Failure),
    Cancelled,
    Timeout(TimeoutKind),
    StoppedUnexpectedly,
    AdapterPanicked { maybe_started: bool },
    SchedulerPanicked { maybe_started: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Update { turn_id: TurnId, update: Update },
    Ended { turn_id: TurnId, reason: EndReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Cancelled,
    Timeout(TimeoutKind),
}

struct Running {
    id: TurnId,
    exchange: Option<Box<dyn Exchange>>,
    timeouts: Option<Timeouts>,
    stop_grace: Duration,
    started_at: Instant,
    last_work: Instant,
    started: bool,
    stop: Option<StopReason>,
    stop_limit: Option<Instant>,
}

impl Running {
    fn timeout(&self, now: Instant) -> Option<TimeoutKind> {
        let limits = self.timeouts?;
        if now.saturating_duration_since(self.started_at) >= limits.max_turn {
            return Some(TimeoutKind::Absolute);
        }
        if self.started {
            (now.saturating_duration_since(self.last_work) >= limits.idle)
                .then_some(TimeoutKind::Idle)
        } else {
            (now.saturating_duration_since(self.started_at) >= limits.start)
                .then_some(TimeoutKind::Start)
        }
    }

    fn recognized_work(update: &Update) -> bool {
        matches!(
            update,
            Update::Started { .. }
                | Update::Activity
                | Update::Delta(_)
                | Update::Source(_)
                | Update::Usage(_)
        )
    }

    fn cancel_exchange(&mut self, grace: Duration) -> Result<(), ()> {
        let Some(exchange) = self.exchange.as_mut() else {
            return Ok(());
        };
        catch_unwind(AssertUnwindSafe(|| exchange.cancel(grace))).map_err(|_| ())
    }

    fn next(&mut self, deadline: Instant) -> Result<Option<Update>, ()> {
        let Some(exchange) = self.exchange.as_mut() else {
            return Ok(None);
        };
        catch_unwind(AssertUnwindSafe(|| exchange.next(deadline))).map_err(|_| ())
    }

    fn drop_exchange(&mut self) {
        if let Some(exchange) = self.exchange.take() {
            let _ = catch_unwind(AssertUnwindSafe(|| drop(exchange)));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.drop_exchange();
    }
}

pub struct Scheduler {
    next_id: TurnId,
    running: Vec<Running>,
    #[cfg(any(test, feature = "test-hooks"))]
    panic_next_poll: bool,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            running: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            panic_next_poll: false,
        }
    }

    pub fn start(
        &mut self,
        exchange: Box<dyn Exchange>,
        timeouts: Option<Timeouts>,
        status_stop_grace: Duration,
    ) -> TurnId {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        let now = Instant::now();
        self.running.push(Running {
            id,
            exchange: Some(exchange),
            timeouts,
            stop_grace: timeouts.map_or(status_stop_grace, |limits| limits.stop_grace),
            started_at: now,
            last_work: now,
            started: false,
            stop: None,
            stop_limit: None,
        });
        id
    }

    pub fn contains(&self, id: TurnId) -> bool {
        self.running.iter().any(|running| running.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    pub fn cancel(&mut self, id: TurnId) -> bool {
        let Some(running) = self.running.iter_mut().find(|running| running.id == id) else {
            return false;
        };
        if running.stop.is_none() {
            let grace = running.stop_grace;
            if running.cancel_exchange(grace).is_err() {
                running.stop = Some(StopReason::Cancelled);
                running.stop_limit = Some(Instant::now());
                return true;
            }
            running.stop = Some(StopReason::Cancelled);
            let now = Instant::now();
            running.stop_limit = Some(now.checked_add(grace + STOP_SLACK).unwrap_or(now));
        }
        true
    }

    pub fn shutdown(&mut self, grace: Duration) {
        for running in &mut self.running {
            if running.stop.is_none() {
                let _ = running.cancel_exchange(grace);
                running.stop = Some(StopReason::Cancelled);
                let now = Instant::now();
                running.stop_limit = Some(now.checked_add(grace + STOP_SLACK).unwrap_or(now));
            }
        }
    }

    pub fn poll(&mut self, slice: Duration) -> Vec<Event> {
        #[cfg(any(test, feature = "test-hooks"))]
        if std::mem::take(&mut self.panic_next_poll) {
            panic!("injected scheduler panic");
        }

        let mut out = Vec::new();
        let mut index = 0;
        while index < self.running.len() {
            let now = Instant::now();
            if let Some(kind) = self.running[index].timeout(now) {
                if self.running[index].stop.is_none() {
                    let grace = self.running[index].stop_grace;
                    let _ = self.running[index].cancel_exchange(grace);
                    self.running[index].stop = Some(StopReason::Timeout(kind));
                    self.running[index].stop_limit =
                        Some(now.checked_add(grace + STOP_SLACK).unwrap_or(now));
                }
            }

            let slice_end = now.checked_add(slice).unwrap_or(now);
            let mut finished = None;
            loop {
                let now = Instant::now();
                if self.running[index]
                    .stop_limit
                    .is_some_and(|limit| now >= limit)
                {
                    finished = Some(match self.running[index].stop {
                        Some(StopReason::Timeout(kind)) => EndReason::Timeout(kind),
                        _ => EndReason::Cancelled,
                    });
                    break;
                }

                let update = match self.running[index].next(now) {
                    Ok(Some(update)) => update,
                    Ok(None) => break,
                    Err(()) => {
                        finished = Some(EndReason::AdapterPanicked {
                            maybe_started: true,
                        });
                        break;
                    }
                };

                if Running::recognized_work(&update) {
                    self.running[index].last_work = Instant::now();
                }
                if matches!(update, Update::Started { .. }) {
                    self.running[index].started = true;
                }

                if update.is_terminal() {
                    finished = Some(match (self.running[index].stop, update) {
                        (Some(StopReason::Timeout(kind)), _) => EndReason::Timeout(kind),
                        (Some(StopReason::Cancelled), _) => EndReason::Cancelled,
                        (None, Update::Completed) => EndReason::Completed,
                        (None, Update::Failed(error)) => EndReason::Failed(error),
                        (None, Update::Stopped) => EndReason::StoppedUnexpectedly,
                        _ => EndReason::StoppedUnexpectedly,
                    });
                    break;
                }

                out.push(Event::Update {
                    turn_id: self.running[index].id,
                    update,
                });
                if Instant::now() >= slice_end {
                    break;
                }
            }

            if let Some(reason) = finished {
                let mut running = self.running.remove(index);
                let id = running.id;
                running.drop_exchange();
                out.push(Event::Ended {
                    turn_id: id,
                    reason,
                });
            } else {
                index += 1;
            }
        }
        out
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_scheduler_panic_for_test(&mut self) {
        self.panic_next_poll = true;
    }

    fn take_all_after_panic(&mut self) -> Vec<Event> {
        self.running
            .drain(..)
            .map(|mut running| {
                let id = running.id;
                running.drop_exchange();
                Event::Ended {
                    turn_id: id,
                    reason: EndReason::SchedulerPanicked {
                        maybe_started: true,
                    },
                }
            })
            .collect()
    }
}

pub struct Supervisor {
    scheduler: Scheduler,
    generation: u64,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            scheduler: Scheduler::new(),
            generation: 1,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn start(
        &mut self,
        exchange: Box<dyn Exchange>,
        timeouts: Option<Timeouts>,
        status_stop_grace: Duration,
    ) -> TurnId {
        self.scheduler.start(exchange, timeouts, status_stop_grace)
    }

    pub fn contains(&self, id: TurnId) -> bool {
        self.scheduler.contains(id)
    }

    pub fn is_empty(&self) -> bool {
        self.scheduler.is_empty()
    }

    pub fn cancel(&mut self, id: TurnId) -> bool {
        self.scheduler.cancel(id)
    }

    pub fn shutdown(&mut self, grace: Duration) {
        self.scheduler.shutdown(grace);
    }

    pub fn poll(&mut self, slice: Duration) -> Vec<Event> {
        match catch_unwind(AssertUnwindSafe(|| self.scheduler.poll(slice))) {
            Ok(events) => events,
            Err(_) => {
                let mut events = self.scheduler.take_all_after_panic();
                self.scheduler = Scheduler::new();
                self.generation = self.generation.saturating_add(1);
                events.shrink_to_fit();
                events
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_scheduler_panic_for_test(&mut self) {
        self.scheduler.inject_scheduler_panic_for_test();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct Scripted {
        updates: VecDeque<Update>,
        panic_next: bool,
    }

    impl Scripted {
        fn new(updates: impl IntoIterator<Item = Update>) -> Self {
            Self {
                updates: updates.into_iter().collect(),
                panic_next: false,
            }
        }

        fn panics() -> Self {
            Self {
                updates: VecDeque::new(),
                panic_next: true,
            }
        }
    }

    impl Exchange for Scripted {
        fn next(&mut self, _deadline: Instant) -> Option<Update> {
            if std::mem::take(&mut self.panic_next) {
                panic!("adapter panic");
            }
            self.updates.pop_front()
        }

        fn cancel(&mut self, _grace: Duration) {
            self.updates = VecDeque::from([Update::Stopped]);
        }
    }

    fn limits() -> Timeouts {
        Timeouts {
            start: Duration::from_secs(1),
            idle: Duration::from_secs(1),
            max_turn: Duration::from_secs(2),
            stop_grace: Duration::ZERO,
        }
    }

    #[test]
    fn ids_never_repeat_within_a_scheduler_lifetime() {
        let mut scheduler = Scheduler::new();
        let a = scheduler.start(Box::new(Scripted::new([Update::Completed])), Some(limits()), Duration::ZERO);
        let _ = scheduler.poll(Duration::from_millis(1));
        let b = scheduler.start(Box::new(Scripted::new([Update::Completed])), Some(limits()), Duration::ZERO);
        assert_ne!(a, b);
    }

    #[test]
    fn an_adapter_panic_ends_only_its_turn() {
        let mut scheduler = Scheduler::new();
        let bad = scheduler.start(Box::new(Scripted::panics()), Some(limits()), Duration::ZERO);
        let good = scheduler.start(
            Box::new(Scripted::new([Update::Started { conversation_id: None }, Update::Completed])),
            Some(limits()),
            Duration::ZERO,
        );
        let events = scheduler.poll(Duration::from_millis(1));
        assert!(events.iter().any(|event| matches!(event, Event::Ended { turn_id, reason: EndReason::AdapterPanicked { .. } } if *turn_id == bad)));
        assert!(events.iter().any(|event| matches!(event, Event::Ended { turn_id, reason: EndReason::Completed } if *turn_id == good)));
    }

    #[test]
    fn a_scheduler_panic_ends_every_owned_turn_and_recovers() {
        let mut supervisor = Supervisor::new();
        let first = supervisor.start(Box::new(Scripted::new([])), Some(limits()), Duration::ZERO);
        supervisor.inject_scheduler_panic_for_test();
        let events = supervisor.poll(Duration::ZERO);
        assert!(events.iter().any(|event| matches!(event, Event::Ended { turn_id, reason: EndReason::SchedulerPanicked { .. } } if *turn_id == first)));
        assert_eq!(supervisor.generation(), 2);

        let second = supervisor.start(Box::new(Scripted::new([Update::Completed])), Some(limits()), Duration::ZERO);
        assert!(supervisor.poll(Duration::ZERO).iter().any(|event| matches!(event, Event::Ended { turn_id, reason: EndReason::Completed } if *turn_id == second)));
    }
}
