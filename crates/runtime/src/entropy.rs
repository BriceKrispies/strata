//! Random bytes as the services read them.
//!
//! Every random value a service makes for its domain (an identifier, a token, a sealing nonce, a
//! key) is read through [`fill`], never from the operating system directly, so that a
//! deterministic simulation of the service can supply seeded bytes on its own thread with
//! [`on_this_thread`]. With no source installed on the calling thread, which is always the case
//! in a deployed process, [`fill`] is exactly `getrandom`.
//!
//! The override is per thread and lasts while its guard lives: it cannot reach another thread,
//! and nothing a deployed process runs installs one.

use std::cell::RefCell;

/// A source of bytes that stands in for the operating system's on one thread.
pub type Source = Box<dyn FnMut(&mut [u8])>;

thread_local! {
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
}

/// Fill `bytes` with random bytes: from the source installed on this thread, if any, otherwise
/// from the operating system.
pub fn fill(bytes: &mut [u8]) -> Result<(), getrandom::Error> {
    let supplied = SOURCE.with(|source| match source.try_borrow_mut() {
        Ok(mut source) => match source.as_mut() {
            Some(source) => {
                source(bytes);
                true
            }
            None => false,
        },
        Err(_) => false,
    });
    if supplied {
        Ok(())
    } else {
        getrandom::getrandom(bytes)
    }
}

/// Restores the source that was installed before [`on_this_thread`] when dropped.
pub struct Installed {
    previous: Option<Source>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SOURCE.with(|source| *source.borrow_mut() = previous);
    }
}

/// Make [`fill`] read from `source` on the calling thread until the returned guard is dropped.
pub fn on_this_thread(source: Source) -> Installed {
    let previous = SOURCE.with(|slot| slot.borrow_mut().replace(source));
    Installed { previous }
}
