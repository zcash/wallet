//! Input and tick event handling for the TUI.

use std::time::Duration;

use crossterm::event::{Event as CtEvent, EventStream, KeyEvent};
use futures::{FutureExt, StreamExt};
use tokio::time::{Interval, MissedTickBehavior, interval};

/// An event delivered to the TUI event loop.
pub(super) enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// A periodic tick fired; the UI should refresh time-sensitive data.
    Tick,
    /// The terminal was resized.
    Resize,
}

/// A combined stream of terminal input events and periodic ticks.
pub(super) struct EventSource {
    reader: EventStream,
    ticker: Interval,
}

impl EventSource {
    /// Creates an event source that ticks every `tick_rate`.
    pub(super) fn new(tick_rate: Duration) -> Self {
        let mut ticker = interval(tick_rate);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        Self {
            reader: EventStream::new(),
            ticker,
        }
    }

    /// Waits for the next event (a key press, a resize, or a tick).
    ///
    /// Non-key terminal events other than resize are skipped transparently.
    pub(super) async fn next(&mut self) -> Event {
        loop {
            tokio::select! {
                _ = self.ticker.tick() => return Event::Tick,
                maybe = self.reader.next().fuse() => {
                    match maybe {
                        Some(Ok(CtEvent::Key(key))) => return Event::Key(key),
                        Some(Ok(CtEvent::Resize(_, _))) => return Event::Resize,
                        // Mouse / focus / paste events: ignore and keep waiting.
                        Some(Ok(_)) => continue,
                        // Read error or end of stream: fall back to ticking so the UI
                        // stays responsive rather than spinning.
                        Some(Err(_)) | None => {
                            self.ticker.tick().await;
                            return Event::Tick;
                        }
                    }
                }
            }
        }
    }
}
