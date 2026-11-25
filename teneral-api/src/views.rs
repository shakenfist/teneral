//! How many views of the store this daemon will hold at once.
//!
//! A view costs the store a thread for as long as it is held, and it
//! holds a read transaction while it is there. So the number of them
//! open at once is a statement about how much of the machine a hundred
//! consumers reading at the same time may take, and it is this layer's
//! to make: the policy and the vocabulary a refusal is expressed in
//! are the control plane's, and the number is a statement about how
//! many concurrent consumers there are supposed to be, which nothing
//! below this knows.
//!
//! The store counts views too, and the two counts are deliberately not
//! the same number. [`teneral_store::BindingStore::live_views`] is
//! every view the store has handed out, whatever opened it, and a
//! datapath sharing this process will one day open its own; the
//! semaphore here counts only the views this server opened. The
//! diagnostics report both, side by side, because a bound enforced on
//! one number and reported on another is the arrangement that notices
//! the day they disagree.
//!
//! # Two ways to want a view, and why they differ
//!
//! A request is refused when there is no permit free, with a named
//! refusal carrying a retry hint rather than a dropped connection: a
//! consumer that asked for one answer too many should be told to come
//! back, which is a thing a client can act on, and a connection error
//! is not.
//!
//! The watch pump waits instead. There is nobody for it to refuse --
//! it serves every stream in the daemon rather than one caller -- and
//! a pump that gave up reading the history would leave every consumer
//! in the daemon silently behind. Waiting is bounded in practice
//! because every view here is opened, used and dropped inside one
//! operation; nothing holds one across a conversation with a client.
//!
//! Both go through one function, so [`teneral_store::BindingStore::read`]
//! is called in exactly one place in this crate and no view can be
//! opened without a permit being held for as long as it lives.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use teneral_store::{BindingStore, Error as StoreError, ReadHandle};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::refusal::{self, Refused};

/// How long a caller refused for want of a view is told to wait.
///
/// A second, because every view this daemon opens is opened, read and
/// dropped inside one operation: a caller that waits for one of those
/// to finish is waiting for a read of the store and not for another
/// consumer to go away. The hint exists so that a client has a number
/// to back off by rather than a reason to reconnect in a loop.
const RETRY_AFTER_SECONDS: u32 = 1;

/// The bound, and the permits that are it.
#[derive(Debug)]
pub(crate) struct Views {
    /// One permit per view this daemon will hold.
    permits: Arc<Semaphore>,
    /// How many that was, for the refusal and for the diagnostics.
    /// Kept beside the semaphore because a semaphore reports what is
    /// free rather than what it started with.
    bound: usize,
}

impl Views {
    /// A bound of `bound` views.
    pub(crate) fn new(bound: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(bound)),
            bound,
        }
    }

    /// A view, or the refusal to hand a caller instead of one.
    ///
    /// What every operation serving a request uses. The refusal is the
    /// named one with a retry hint, not a failure: this is the daemon
    /// being busy rather than the request being wrong.
    pub(crate) async fn refusing(&self, store: &dyn BindingStore) -> Result<View, Refused> {
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| -> Refused {
                Box::new(refusal::too_many_views(
                    self.held(),
                    self.bound,
                    RETRY_AFTER_SECONDS,
                ))
            })?;

        self.open(store, permit)
            .await
            .map_err(|error| -> Refused { Box::new(refusal::for_store_error(&error)) })
    }

    /// A view, waiting for a permit for as long as it takes.
    ///
    /// What the watch pump uses, for the reason this module's
    /// documentation gives: there is nobody to refuse, and a pump that
    /// stopped reading would leave every stream in the daemon behind
    /// without saying so. A caller that cannot afford to wait selects
    /// on this against whatever else it is waiting for.
    pub(crate) async fn waiting(&self, store: &dyn BindingStore) -> Result<View, StoreError> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable {
                // The semaphore is closed by nothing in this crate, so
                // this is a state the code cannot reach rather than one
                // it recovers from -- and a read that cannot be opened
                // is the honest thing to report either way.
                context: "open a view of the store, the bound on them having been closed"
                    .to_owned(),
            })?;

        self.open(store, permit).await
    }

    /// How many views this daemon is holding right now.
    ///
    /// Derived from what the semaphore has left rather than counted
    /// separately, so there is no second number to fall out of step
    /// with the permits.
    pub(crate) fn held(&self) -> usize {
        self.bound.saturating_sub(self.permits.available_permits())
    }

    /// How many this daemon will hold at once.
    pub(crate) fn bound(&self) -> usize {
        self.bound
    }

    /// Open a view against a permit already held.
    ///
    /// The one call to the store's `read` in this crate. Everything
    /// else asks here, which is what makes "a view is never opened
    /// without a permit" a property of the code rather than a rule
    /// each caller remembers.
    async fn open(
        &self,
        store: &dyn BindingStore,
        permit: OwnedSemaphorePermit,
    ) -> Result<View, StoreError> {
        let handle = store.read().await?;

        Ok(View {
            handle,
            _permit: permit,
        })
    }
}

/// A view of the store, holding a permit for as long as it exists.
///
/// Dereferences to the handle, so a caller reads through it exactly as
/// it would read through the store's own: the permit is a thing this
/// type carries rather than a thing a caller has to know about.
pub(crate) struct View {
    /// The view itself. Declared before the permit so that it is
    /// dropped first: dropping it ends the read transaction and joins
    /// the store's thread, and the permit must not be handed to the
    /// next caller until that has happened. Fields drop in declaration
    /// order, which is what makes the order here the order it is.
    handle: Box<dyn ReadHandle>,
    /// The permit, released when this view is dropped, so the bound
    /// counts views that are open rather than views that were asked
    /// for.
    _permit: OwnedSemaphorePermit,
}

impl Deref for View {
    type Target = dyn ReadHandle;

    fn deref(&self) -> &Self::Target {
        &*self.handle
    }
}

impl DerefMut for View {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::Views;
    use crate::proto::RefusalCode;
    use crate::testing::Directory;
    use teneral_store::BindingStore;

    #[tokio::test]
    async fn the_view_after_the_last_permit_is_refused_and_the_store_is_let_go_of() {
        let directory = Directory::new("bound");
        let store = directory.store().await;
        let views = Views::new(2);

        let first = views.refusing(&*store).await.expect("the first view");
        let second = views.refusing(&*store).await.expect("the second view");

        assert_eq!(views.held(), 2);
        assert_eq!(store.live_views(), 2);

        // The view after the last permit, which is the one that has to
        // be refused rather than queued: a caller waiting for a view
        // behind a bound it cannot see is a caller whose request has
        // no answer and no refusal either.
        let refusal = match views.refusing(&*store).await {
            Ok(_) => panic!("a view was opened past the bound"),
            Err(refusal) => refusal,
        };

        assert_eq!(refusal.code, RefusalCode::TooManyViews as i32);
        assert_eq!(
            refusal.retry_after_seconds,
            Some(super::RETRY_AFTER_SECONDS)
        );
        assert!(
            refusal.detail.contains('2'),
            "the refusal says what the bound is: {}",
            refusal.detail
        );

        // And the permit is released by dropping the view rather than
        // by anything remembering to, which is what makes the count
        // return to zero.
        drop(first);
        drop(second);

        assert_eq!(views.held(), 0);
        assert_eq!(
            store.live_views(),
            0,
            "the store is still holding a view this daemon let go of"
        );

        let again = views.refusing(&*store).await;
        assert!(again.is_ok(), "the bound did not come back");
    }
}
