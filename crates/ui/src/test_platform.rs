//! Real platforms for tests that need the native text system or executor
//! rather than `TestAppContext`'s simulation.
//!
//! On macOS, `MacPlatform::new` reads the current keyboard layout through
//! the Text Input Sources API, and HIToolbox aborts the whole process when
//! two threads call that API at the same time, which the parallel test
//! harness does as soon as two such tests overlap. Every real platform a
//! test builds therefore comes from here, where construction is serialized.
//! `App::new` reads the layout again, so a test that runs a headless
//! `Application` on macOS must hold [`serialized`] for the whole run; the
//! `gpui_platform::headless().run(..)` tests are Linux-only today.
use std::sync::{Mutex, MutexGuard, PoisonError};

static PLATFORM: Mutex<()> = Mutex::new(());

fn serialized() -> MutexGuard<'static, ()> {
    PLATFORM.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn background_executor() -> gpui::BackgroundExecutor {
    let _guard = serialized();
    gpui_platform::background_executor()
}

pub(crate) fn current_platform(headless: bool) -> std::rc::Rc<dyn gpui_platform::Platform> {
    let _guard = serialized();
    gpui_platform::current_platform(headless)
}
