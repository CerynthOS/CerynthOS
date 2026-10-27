//! A scheduler must be ready to dispatch before BPF takes ownership of tasks.
pub fn prepare_then_attach<S, P, L, E>(
    state: &mut S,
    prepare: impl FnOnce(&mut S) -> Result<P, E>,
    attach: impl FnOnce(&mut S) -> Result<L, E>,
) -> Result<(P, L), E> {
    let prepared = prepare(state)?;
    let link = attach(state)?;
    Ok((prepared, link))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct State { active: bool, prepared: bool, attaches: usize }

    #[test]
    fn blocking_preparation_finishes_before_scheduler_ownership() {
        let mut state = State::default();
        let result = prepare_then_attach(&mut state, |s| {
            if s.active { return Err("setup dependency needs this scheduler: deadlock"); }
            s.prepared = true;
            Ok(())
        }, |s| {
            assert!(s.prepared);
            s.active = true;
            s.attaches += 1;
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(state.attaches, 1);
    }

    #[test]
    fn failed_preparation_never_attaches() {
        let mut state = State::default();
        let result = prepare_then_attach(&mut state, |_| Err::<(),_>("memory setup failed"), |s| {
            s.attaches += 1;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(state.attaches, 0);
        assert!(!state.active);
    }

    #[test]
    fn failed_attachment_releases_prepared_resources() {
        use std::cell::Cell;
        struct Resource<'a>(&'a Cell<bool>);
        impl Drop for Resource<'_> { fn drop(&mut self) { self.0.set(true); } }
        let released = Cell::new(false);
        let mut state = State::default();
        let result = prepare_then_attach(&mut state, |_| Ok(Resource(&released)),
            |_| Err::<(),_>("attach failed"));
        assert!(result.is_err());
        assert!(released.get());
    }
}
