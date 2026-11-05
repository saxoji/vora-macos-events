use std::rc::Rc;

enum Entry<T> {
    Empty,
    Installing(u64),
    Active(u64, Rc<T>),
}

/// The caller holds its RefCell borrow only while using these methods. Returned
/// Rc values keep native objects alive without retaining the borrow over AppKit
/// or application callbacks.
pub(crate) struct Registry<T> {
    next: u64,
    entry: Entry<T>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            next: 0,
            entry: Entry::Empty,
        }
    }
}

impl<T> Registry<T> {
    pub(crate) fn reserve(&mut self) -> Option<u64> {
        if !matches!(self.entry, Entry::Empty) {
            return None;
        }
        let token = self.next.checked_add(1)?;
        self.next = token;
        self.entry = Entry::Installing(token);
        Some(token)
    }

    pub(crate) fn publish(&mut self, token: u64, value: Rc<T>) -> bool {
        if !matches!(self.entry, Entry::Installing(current) if current == token) {
            return false;
        }
        self.entry = Entry::Active(token, value);
        true
    }

    pub(crate) fn active(&self) -> Option<(u64, Rc<T>)> {
        match &self.entry {
            Entry::Active(token, value) => Some((*token, value.clone())),
            _ => None,
        }
    }

    pub(crate) fn remove(&mut self, token: u64) -> Option<Rc<T>> {
        let matches = match &self.entry {
            Entry::Installing(current) | Entry::Active(current, _) => *current == token,
            Entry::Empty => false,
        };
        if !matches {
            return None;
        }
        match std::mem::replace(&mut self.entry, Entry::Empty) {
            Entry::Active(_, value) => Some(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installing_is_not_visible_and_cannot_be_reentered() {
        let mut registry = Registry::<u8>::default();
        let token = registry.reserve().unwrap();
        assert!(registry.active().is_none());
        assert!(registry.reserve().is_none());
        assert!(!registry.publish(token + 1, Rc::new(1)));
        registry.remove(token);
        assert!(registry.reserve().is_some());
    }

    #[test]
    fn late_guard_cannot_remove_replacement_and_callback_retains_state() {
        let mut registry = Registry::default();
        let token = registry.reserve().unwrap();
        assert!(registry.publish(token, Rc::new(1)));
        let retained = registry.active().unwrap().1;
        registry.remove(token);
        assert!(registry.active().is_none());
        assert_eq!(*retained, 1);
        let next = registry.reserve().unwrap();
        assert_ne!(next, token);
        assert!(registry.publish(next, Rc::new(2)));
        assert!(registry.remove(token).is_none());
        assert_eq!(*registry.active().unwrap().1, 2);
    }
}
