use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static BEFORE_SYNC_IDEMPOTENT: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

pub struct ArmedFault;

impl Drop for ArmedFault {
    fn drop(&mut self) {
        BEFORE_SYNC_IDEMPOTENT.with(|slot| slot.replace(None));
    }
}

pub fn before_sync_idempotent_check(f: impl Fn() + 'static) -> ArmedFault {
    BEFORE_SYNC_IDEMPOTENT.with(|slot| slot.replace(Some(Rc::new(f))));
    ArmedFault
}

pub(crate) fn fire_before_sync_idempotent_check() {
    let armed = BEFORE_SYNC_IDEMPOTENT.with(|slot| slot.borrow().clone());
    if let Some(armed) = armed {
        armed();
    }
}
