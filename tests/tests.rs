#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use mutation_monitor::{ Mutate, OnMutate };

    #[test]
    fn notifies_on_change() {
        let seen: Rc<RefCell<Vec<Mutate<i32>>>> = Rc::new(RefCell::new(vec![]));
        let s2 = seen.clone();
        let on = OnMutate::new(0, move |evt| s2.borrow_mut().push(evt.clone()));

        on.with_mut(None, |v| *v = 42);
        on.with_mut(Some("answer".into()), |v| *v = 43);

        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].old, 0);
        assert_eq!(seen[0].new, 42);
        assert!(seen[0].tag.is_none());
        assert_eq!(seen[1].tag.as_deref(), Some("answer"));
    }

    #[test]
    fn notifies_on_guard_drop() {
        let seen: Rc<RefCell<Vec<Mutate<String>>>> = Rc::new(RefCell::new(vec![]));
        let s2 = seen.clone();
        let on = OnMutate::new(String::from("a"), move |evt| s2.borrow_mut().push(evt.clone()));

        {
            let mut g = on.with_tag("append");
            g.push('b');
        }

        {
            let g = on.with_guard();
            let _ = g.len();
        }

        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(on.get_val(), "ab");
    }

    #[test]
    fn reentrant_notify_is_safe() {
        // Callback mutates again; queue must handle re-entrancy safely
        let holder: Rc<RefCell<Option<OnMutate<i32>>>> = Rc::new(RefCell::new(None));
        let holder2 = holder.clone();

        let on = OnMutate::new(0, move |evt: &Mutate<i32>| {
            if let Some(ref on_inner) = *holder2.borrow() {
                if evt.new < 3 {
                    on_inner.with_mut(None, |v| *v += 1);
                }
            }
        });

        *holder.borrow_mut() = Some(on);
        let on = holder.borrow();
        let on = on.as_ref().unwrap();

        on.with_mut(None, |v| *v += 1);
        assert_eq!(on.get_val(), 3);
    }
}
