//! Threaded entry point for async/server consumers.
//!
//! The service owns its supervisor on a dedicated thread. A channel closing
//! before a terminal event is converted by the turn handle into one synthetic
//! runtime-loss ending, preserving the exactly-once consumer contract.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use provider_runtime_scheduler::{EndReason, Event, Supervisor, TurnId};
use runtime_core::exchange::{Exchange, Timeouts};

pub trait TurnFactory: 'static {
    fn start(&mut self, request: Vec<u8>) -> Result<(Box<dyn Exchange>, Option<Timeouts>), String>;
}

enum Command {
    Start {
        request: Vec<u8>,
        reply: Sender<Result<(TurnId, Receiver<Event>), String>>,
    },
    Cancel(TurnId),
    Stop,
}

pub struct Runtime {
    commands: Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Runtime {
    pub fn start<F>(factory: F) -> Self
    where
        F: FnOnce() -> Box<dyn TurnFactory> + Send + 'static,
    {
        let (commands, receiver) = mpsc::channel();
        let thread = thread::spawn(move || service_loop(factory(), receiver));
        Self {
            commands,
            thread: Some(thread),
        }
    }

    pub fn start_turn(&self, request: Vec<u8>) -> Result<Turn, String> {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Start { request, reply })
            .map_err(|_| "runtime service is not running".to_owned())?;
        let (id, events) = answer
            .recv()
            .map_err(|_| "runtime service stopped while starting a turn".to_owned())??;
        Ok(Turn {
            id,
            events,
            commands: self.commands.clone(),
            ended: false,
        })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct Turn {
    id: TurnId,
    events: Receiver<Event>,
    commands: Sender<Command>,
    ended: bool,
}

impl Turn {
    pub fn id(&self) -> TurnId {
        self.id
    }

    pub fn cancel(&self) {
        let _ = self.commands.send(Command::Cancel(self.id));
    }

    pub fn next(&mut self) -> Option<Event> {
        if self.ended {
            return None;
        }
        match self.events.recv() {
            Ok(event @ Event::Ended { .. }) => {
                self.ended = true;
                Some(event)
            }
            Ok(event) => Some(event),
            Err(_) => {
                self.ended = true;
                Some(Event::Ended {
                    turn_id: self.id,
                    reason: EndReason::SchedulerPanicked {
                        maybe_started: true,
                    },
                })
            }
        }
    }
}

fn service_loop(mut factory: Box<dyn TurnFactory>, commands: Receiver<Command>) {
    let mut supervisor = Supervisor::new();
    let mut outputs: HashMap<TurnId, Sender<Event>> = HashMap::new();

    loop {
        match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(Command::Start { request, reply }) => {
                let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| factory.start(request)));
                match started {
                    Ok(Ok((exchange, limits))) => {
                        let (sender, events) = mpsc::channel();
                        let id = supervisor.start(exchange, limits, Duration::from_millis(250));
                        outputs.insert(id, sender);
                        let _ = reply.send(Ok((id, events)));
                    }
                    Ok(Err(error)) => {
                        let _ = reply.send(Err(error));
                    }
                    Err(_) => {
                        let _ = reply.send(Err("provider factory panicked".to_owned()));
                    }
                }
            }
            Ok(Command::Cancel(id)) => {
                let _ = supervisor.cancel(id);
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                supervisor.shutdown(Duration::from_millis(250));
                while !supervisor.is_empty() {
                    dispatch(&mut supervisor, &mut outputs);
                    thread::sleep(Duration::from_millis(1));
                }
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        dispatch(&mut supervisor, &mut outputs);
    }
}

fn dispatch(supervisor: &mut Supervisor, outputs: &mut HashMap<TurnId, Sender<Event>>) {
    for event in supervisor.poll(Duration::from_millis(5)) {
        let id = match &event {
            Event::Update { turn_id, .. } | Event::Ended { turn_id, .. } => *turn_id,
        };
        let ended = matches!(event, Event::Ended { .. });
        if let Some(output) = outputs.get(&id) {
            let _ = output.send(event);
        }
        if ended {
            outputs.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Instant;

    use runtime_core::exchange::Update;

    use super::*;

    struct Factory;

    struct One(VecDeque<Update>);

    impl Exchange for One {
        fn next(&mut self, _deadline: Instant) -> Option<Update> {
            self.0.pop_front()
        }

        fn cancel(&mut self, _grace: Duration) {
            self.0 = VecDeque::from([Update::Stopped]);
        }
    }

    impl TurnFactory for Factory {
        fn start(&mut self, _request: Vec<u8>) -> Result<(Box<dyn Exchange>, Option<Timeouts>), String> {
            Ok((
                Box::new(One(VecDeque::from([
                    Update::Started { conversation_id: None },
                    Update::Completed,
                ]))),
                Some(Timeouts {
                    start: Duration::from_secs(1),
                    idle: Duration::from_secs(1),
                    max_turn: Duration::from_secs(2),
                    stop_grace: Duration::ZERO,
                }),
            ))
        }
    }

    #[test]
    fn service_turn_ends_once() {
        let runtime = Runtime::start(|| Box::new(Factory));
        let mut turn = runtime.start_turn(b"hello".to_vec()).unwrap();
        let mut ended = 0;
        while let Some(event) = turn.next() {
            if matches!(event, Event::Ended { .. }) {
                ended += 1;
            }
        }
        assert_eq!(ended, 1);
    }
}
