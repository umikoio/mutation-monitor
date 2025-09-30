//! # Mutation Monitor
//!
//! Observe mutations of a value and generate a callback when the value has mutated.
//!
//! Have you ever asked yourself if Rust is able to mutate? Well, now we can watch to see how these mutations change. This is a very simple crate with minimal intrustion to your codebase that watches for any mutations within your defined structures.
//!
//! We watch for changes via `OnChange<T>`, which stores your value inside a `RefCell<T>`, to later recall the data you provided. Every time your data mutates, it'll be clone the "old" value, allow you to finish mutating, and then checks with `PartialEq` to validate the data was actually modified. If the data is successfully changed, a `Change<T>` event is created with the following values for reference: `old`, `new`, `tag`. And beyond this, all events are queued. Which means nothing is delivered until all borrows are released. So here's hoping we don't see a `BorrowMutError`.
//!
//! | Function                         | Description                                                      |
//! |----------------------------------|------------------------------------------------------------------|
//! | `get_val()`                      | Get a clone of the current value                                 |
//! | `with_guard()`                   | Returns a guard; when it drops, we compare old vs new and notify |
//! | `replace(new_value: T)`          | Replace the entire value; notify if different                    |
//! | `with_tag(tag: String)`          | Add a context tage during push, not intial mutation              |
//! | `with_mut<R>(tag: String, f: T)` | Mutate; notify once if changed + add a context tag if applicable |
//!
//! NOTE: This API list is a "dumbed down" version of all supported functions, but it should give a high level overview of what to expect when using the library.
//!

/*
    Author: Umiko (https://github.com/umikoio)
    Project: Mutation Monitor (https://github.com/umikoio/mutation-monitor)
*/

use std::cell::{ Cell, RefCell, RefMut };
use std::fmt;

/// Monitor mutations via a struct to contain the data
#[derive(Clone, Debug, PartialEq)]
pub struct Mutate<T: Clone + PartialEq> {
    pub old: T,
    pub new: T,
    pub tag: Option<String>,
}

impl<T: Clone + PartialEq> Mutate<T> {
    fn new(old: T, new: T, tag: Option<String>) -> Self
    {
        Self { old, new, tag }
    }
}

/// Public observable wrapper for mutations
///
/// We maintain borrow checks (to avoid BorrowMutError) by draining a queue, this way we never make a call while a borrow is held
///
/// Full type implementation: `impl<T: Clone + PartialEq> OnMutate<T> {}`
///
pub struct OnMutate<T: Clone + PartialEq> {
    mut_value: RefCell<T>, // Actual value being ingested
    callback_ref: RefCell<Option<Box<dyn FnMut(&Mutate<T>) + 'static>>>, // Callback for the ingested value
    queue: RefCell<Vec<Mutate<T>>>, // Simple queue for maintaing incoming data
    draining: Cell<bool>, // Is the queue currently draining?
}

impl<T: Clone + PartialEq> fmt::Debug for OnMutate<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OnMutate")
            .field("mut_value", &"<value>")
            .field("callback_ref", &"<callback>")
            .field("queue", &"<queue>")
            .field("draining", &"<draining>")
            .finish()
    }
}

/// Primary implementation for entire mutation monitoring
impl<T: Clone + PartialEq> OnMutate<T> {
    /// New data being ingested
    pub fn new<F>(value: T, callback: F) -> Self
    where F: FnMut(&Mutate<T>) + 'static
    {
        Self {
            mut_value: RefCell::new(value),
            callback_ref: RefCell::new(Some(Box::new(callback))),
            queue: RefCell::new(Vec::new()),
            draining: Cell::new(false),
        }
    }

    /// Get the current mutated value
    pub fn get_val(&self) -> T {
        self.mut_value.borrow().clone()
    }

    /// Push a new event to `queue_event`, if it actually changed
    pub fn replace(&self, new_value: T) {
        let mut current = self.mut_value.borrow_mut();

        if *current != new_value {
            let new_event = Mutate::new(current.clone(), new_value.clone(), None);
            *current = new_value;

            // Release before pushing to queue (this including draining the queue if applicable)
            drop(current);
            self.queue_event(new_event);
        }
    }

    /// Begin mutation detection, notify if changed. Also comes with a non-intrusive tag for categorizing
    pub fn with_mut<R>(&self, tag: impl Into<Option<String>>, f: impl FnOnce(&mut T) -> R) -> R {
        let tag = tag.into();

        // We clone `old` in its own scope so the immutable borrow is dropped
        // This needs to happen before we try to take a new mutable borrow
        let old = {
            let b = self.mut_value.borrow();
            b.clone()
        };

        let mut borrow = self.mut_value.borrow_mut();
        let out = f(&mut borrow);
        let value_mutated = *borrow != old;
        let new_snapshot = borrow.clone();

        // Release before pushing to queue (this including draining the queue if applicable)
        drop(borrow);

        // If the borrowed value is not identical to the old value, we push to the queue
        if value_mutated {
            self.queue_event(Mutate::new(old, new_snapshot, tag));
        }

        out
    }

    /// A monitoring guard that notifies when/if a value is mutated or changed during the drop
    pub fn with_guard(&self) -> OnMutationChange<'_, T> {
        // We clone "old" in its own scope so the immutable borrow is dropped
        let old = {
            let b = self.mut_value.borrow();
            b.clone()
        };

        OnMutationChange {
            owner: self,
            old,
            borrow: Some(self.mut_value.borrow_mut()),
            tag: None,
        }
    }

    /// A self-contained function for including a tag (outside of `with_mut()`)
    pub fn with_tag(&self, tag: impl Into<String>) -> OnMutationChange<'_, T> {
        // We clone "old" in its own scope so the immutable borrow is dropped
        let old = {
            let b = self.mut_value.borrow();
            b.clone()
        };

        OnMutationChange {
            owner: self,
            old,
            borrow: Some(self.mut_value.borrow_mut()),
            tag: Some(tag.into()),
        }
    }

    /// Queue an event and drain if not already draining
    fn queue_event(&self, new_event: Mutate<T>) {
        self.queue.borrow_mut().push(new_event);
        self.drain_queue();
    }

    /// Drain queued events without maintaining any `RefCell` borrows
    fn drain_queue(&self) {
        // Already draining, return
        if self.draining.replace(true) {
            return;
        }

        // We'll keep taking a snapshot of the queue and invoking without holding borrows.
        loop {
            // Construct the current batch/queue
            let batch = {
                let mut q = self.queue.borrow_mut();
                if q.is_empty() { break; }
                std::mem::take(&mut *q)
            };

            // Extract the callback references
            let mut callback_opt = {
                let mut slot = self.callback_ref.borrow_mut();
                slot.take()
            };

            for new_event in batch {
                if let Some(ref mut callback_ref) = callback_opt {
                    (callback_ref)(&new_event);
                }
            }

            // Restore the callback references if it wasn't replaced during callback
            let mut slot = self.callback_ref.borrow_mut();

            if slot.is_none() {
                *slot = callback_opt;
            }
        }

        // We're done draining
        self.draining.set(false);
    }
}

pub struct OnMutationChange<'a, T: Clone + PartialEq> {
    owner: &'a OnMutate<T>,
    old: T,
    borrow: Option<RefMut<'a, T>>,
    tag: Option<String>,
}

// Dereferences the value
impl<'a, T: Clone + PartialEq> std::ops::Deref for OnMutationChange<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let rm = self.borrow.as_ref().expect("released");
        &*rm
    }
}

// Mutably dereferences the value
impl<'a, T: Clone + PartialEq> std::ops::DerefMut for OnMutationChange<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let rm = self.borrow.as_mut().expect("released");
        &mut *rm
    }
}

// Executes the destructor for this type
impl<'a, T: Clone + PartialEq> Drop for OnMutationChange<'a, T> {
    fn drop(&mut self) {
        if let Some(borrow) = self.borrow.take() {
            let value_mutated = *borrow != self.old;
            let new_clone = borrow.clone();

            // Release before pushing to queue (this including draining the queue if applicable)
            drop(borrow);

            if value_mutated {
                self.owner.queue_event(Mutate::new(self.old.clone(), new_clone, self.tag.clone()));
            }
        }
    }
}
