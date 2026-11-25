//! Stops scroll events going round between the browser and a page without end.
//!
//! WebKitGTK puts a scroll event that a page didn't use, such as at the end of
//! the page, back in GTK's queue, for a widget around the page to use instead.
//! It marks the event so that when it comes back, it's passed on rather than
//! sent to the page again. But it can lose that mark: it looks for it from
//! its handler for GTK's start of a scroll as well as for the scroll itself,
//! and the first one clears it, so the second sends the event to the page
//! again, which gives it back again, and so on. Scrolling with a TrackPoint on
//! X11, where each still moment ends a scroll, starts such loops, which
//! together can keep the browser's main thread and the page busy for minutes,
//! so that the page doesn't scroll or answer clicks (issue #40).
//!
//! An event reaches the page once, and comes back once or twice when the page
//! doesn't use it, so once the same event arrives more often than that, it's
//! dropped before it reaches the page.

use std::collections::HashMap;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

/// The most times the same event can be delivered: the first time, and back
/// once for each of the two times WebKit can send it to the page.
const MOST_DELIVERIES: u32 = 3;
/// How long, in event time, deliveries of an event are remembered for.
const REMEMBERED_MS: u32 = 5000;

/// Counts how often each scroll event has been delivered.
#[derive(Default)]
pub struct Deliveries {
    /// By the event's address: its time, and how often it was delivered.
    counts: HashMap<usize, (u32, u32)>,
    newest: u32,
}

impl Deliveries {
    /// Records a delivery of the event at `address` with `time`, and says
    /// whether to let it through.
    pub fn deliver(&mut self, address: usize, time: u32) -> bool {
        if time.wrapping_sub(self.newest) < u32::MAX / 2 {
            self.newest = time;
        }
        let newest = self.newest;
        if self.counts.len() > 1000 {
            self.counts
                .retain(|_, (time, _)| newest.wrapping_sub(*time) <= REMEMBERED_MS);
        }
        let count = match self.counts.get_mut(&address) {
            // The same event, rather than a new one at a freed one's address.
            Some((seen_time, count)) if *seen_time == time => {
                *count += 1;
                *count
            }
            _ => {
                self.counts.insert(address, (time, 1));
                1
            }
        };
        count <= MOST_DELIVERIES
    }
}

/// Drops scroll events that come round too often, before they reach the pages
/// in `window`.
pub fn guard(window: &impl IsA<gtk::Widget>) {
    let deliveries = std::cell::RefCell::new(Deliveries::default());
    let controller = gtk::EventControllerLegacy::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    controller.connect_event(move |_, event| {
        let Some(scroll) = event.downcast_ref::<gdk::ScrollEvent>() else {
            return glib::Propagation::Proceed;
        };
        let address = scroll.as_ptr() as usize;
        if deliveries.borrow_mut().deliver(address, scroll.time()) {
            glib::Propagation::Proceed
        } else {
            crate::debug_stalls::log("dropped a scroll event that came round again");
            glib::Propagation::Stop
        }
    });
    window.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_coming_round_again_is_dropped() {
        let mut deliveries = Deliveries::default();
        // Delivered, then back from the page twice.
        assert!(deliveries.deliver(1, 100));
        assert!(deliveries.deliver(1, 100));
        assert!(deliveries.deliver(1, 100));
        // Going round again.
        assert!(!deliveries.deliver(1, 100));
        assert!(!deliveries.deliver(1, 100));
    }

    #[test]
    fn other_events_are_let_through() {
        let mut deliveries = Deliveries::default();
        for _ in 0..3 {
            assert!(deliveries.deliver(1, 100));
        }
        // Another event, and a new one where a freed one was.
        assert!(deliveries.deliver(2, 100));
        assert!(deliveries.deliver(1, 101));
        assert!(deliveries.deliver(1, 101));
    }

    #[test]
    fn old_events_are_forgotten() {
        let mut deliveries = Deliveries::default();
        for address in 0..2000 {
            deliveries.deliver(address, address as u32);
        }
        deliveries.deliver(5000, 10_000);
        assert!(deliveries.counts.len() < 1000);
        // Event times can wrap around.
        let mut deliveries = Deliveries::default();
        assert!(deliveries.deliver(1, u32::MAX - 5));
        assert!(deliveries.deliver(2, 3));
        assert_eq!(deliveries.newest, 3);
    }
}
