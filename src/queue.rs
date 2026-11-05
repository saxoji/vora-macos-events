use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

struct Pending<T> {
    events: Vec<T>,
    wake: Option<Rc<dyn Fn()>>,
}

struct Shared<T> {
    pending: RefCell<Pending<T>>,
    waking: Cell<bool>,
}

pub(crate) struct Queue<T>(Rc<Shared<T>>);

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self(Rc::new(Shared {
            pending: RefCell::new(Pending {
                events: Vec::new(),
                wake: None,
            }),
            waking: Cell::new(false),
        }))
    }
}

impl<T> Queue<T> {
    pub(crate) fn drain(&self) -> Vec<T> {
        std::mem::take(&mut self.0.pending.borrow_mut().events)
    }

    pub(crate) fn set_wake(&self, wake: impl Fn() + 'static) {
        // The old callback's captures may run user Drop code. Drop them only
        // after the RefCell borrow ends, just like invoking the new callback.
        let old = self.0.pending.borrow_mut().wake.replace(Rc::new(wake));
        drop(old);
    }

    pub(crate) fn push(&self, event: T) {
        let wake = {
            let mut pending = self.0.pending.borrow_mut();
            pending.events.push(event);
            pending.wake.clone()
        };
        let Some(wake) = wake else { return };
        if self.0.waking.replace(true) {
            // Keep every queued event, but coalesce synchronous recursive wake
            // notifications. A wake schedules the UI; it is not event dispatch.
            return;
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&self.0.waking);
        let _ = unwind_boundary(|| wake());
    }
}

/// Never let a Rust panic cross an Objective-C entry point. A panic payload can
/// itself have a panicking destructor, so do not drop it at this boundary.
/// As everywhere in Rust, panic=abort and allocation aborts cannot be recovered.
pub(crate) fn unwind_boundary<T>(f: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            std::mem::forget(payload);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_can_drain_replace_callback_and_enqueue_without_borrow_or_recursion() {
        let q = Queue::default();
        let wake_q = q.clone();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let observed = seen.clone();
        let calls = Rc::new(Cell::new(0));
        let called = calls.clone();
        q.set_wake(move || {
            called.set(called.get() + 1);
            observed.borrow_mut().extend(wake_q.drain());
            wake_q.set_wake(|| {});
            wake_q.push(2);
        });
        q.push(1);
        assert_eq!(*seen.borrow(), [1]);
        assert_eq!(calls.get(), 1);
        assert_eq!(q.drain(), [2]);
    }

    #[test]
    fn each_click_remains_a_distinct_event() {
        let q = Queue::default();
        let calls = Rc::new(Cell::new(0));
        let called = calls.clone();
        q.set_wake(move || called.set(called.get() + 1));
        for event in 0..4 {
            q.push(event);
        }
        assert_eq!(q.drain(), [0, 1, 2, 3]);
        assert_eq!(calls.get(), 4);
        assert!(q.drain().is_empty());
    }

    #[test]
    fn replaced_wake_capture_can_reenter_queue_on_drop() {
        struct OnDrop(Queue<u8>, Rc<Cell<bool>>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.1.set(self.0.drain() == [7]);
            }
        }
        let q = Queue::default();
        q.push(7);
        let observed = Rc::new(Cell::new(false));
        let capture = OnDrop(q.clone(), observed.clone());
        q.set_wake(move || {
            let _ = &capture;
        });
        q.set_wake(|| {});
        assert!(observed.get());
    }

    #[cfg(panic = "unwind")]
    #[test]
    fn panic_does_not_lose_event_or_leave_wake_locked() {
        let q = Queue::default();
        q.set_wake(|| panic!("test callback"));
        q.push(1);
        assert_eq!(q.drain(), [1]);
        let called = Rc::new(Cell::new(false));
        let observed = called.clone();
        q.set_wake(move || observed.set(true));
        q.push(2);
        assert!(called.get());
        assert_eq!(q.drain(), [2]);
    }
}
