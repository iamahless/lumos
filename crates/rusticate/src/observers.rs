//! Model lifecycle observers.
//!
//! Implement [`Observer`] for a model, register it with [`DB::observe`](crate::DB),
//! and hooks fire around every write through that handle (pool and
//! transactions alike — the registry lives on the shared handle).
//! Pre-write hooks (`creating`, `updating`, `saving`, `deleting`,
//! `restoring`) receive `&mut` and may adjust the instance (stamp UUIDs,
//! normalize fields); post-write hooks receive `&` (the row is already
//! persisted, so mutations would be misleading).
//!
//! Every hook receives the current [`Target`](crate::Target), so hooks may
//! themselves query — including on the same transaction the write runs in.
//! Hook dispatch never holds a connection lock while running user code, so
//! this cannot deadlock (covered by test).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;

use crate::db::Target;
use crate::model::Model;
use crate::{Error, Result};

/// Lifecycle hooks for model `M`. Every hook defaults to a no-op — implement
/// only what the model needs. Returning `Err` aborts the write (and fails
/// the enclosing transaction, when any).
///
/// Hook order for each operation follows Eloquent: `saving` wraps both
/// insert and update paths; `creating`/`updating` are path-specific.
/// `target` is where the write runs: query through it to stay inside the
/// same transaction.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{async_trait, Model, Observer, Result, Target};
/// # #[derive(Model)]
/// # #[model(table = "users")]
/// # pub struct User {
/// #     #[model(id)]
/// #     pub uuid: uuid::Uuid,
/// # }
/// # impl User { fn send_welcome(&self) {} }
///
/// pub struct UserObserver;
///
/// #[async_trait]
/// impl Observer<User> for UserObserver {
///     async fn creating(&self, _target: &Target, user: &mut User) -> Result<()> {
///         user.uuid = uuid::Uuid::new_v4();
///         Ok(())
///     }
///
///     async fn created(&self, _target: &Target, user: &User) -> Result<()> {
///         user.send_welcome();
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Observer<M: Model>: Send + Sync {
    /// Before any insert or update. May adjust the instance.
    async fn saving(&self, _target: &Target, _model: &mut M) -> Result<()> {
        Ok(())
    }

    /// After any insert or update was persisted.
    async fn saved(&self, _target: &Target, _model: &M) -> Result<()> {
        Ok(())
    }

    /// Before an insert. May adjust the instance (stamp UUIDs, defaults).
    async fn creating(&self, _target: &Target, _model: &mut M) -> Result<()> {
        Ok(())
    }

    /// After an insert was persisted.
    async fn created(&self, _target: &Target, _model: &M) -> Result<()> {
        Ok(())
    }

    /// Before an update. May adjust the instance.
    async fn updating(&self, _target: &Target, _model: &mut M) -> Result<()> {
        Ok(())
    }

    /// After an update was persisted.
    async fn updated(&self, _target: &Target, _model: &M) -> Result<()> {
        Ok(())
    }

    /// Before a delete or force-delete. May veto by returning `Err`.
    async fn deleting(&self, _target: &Target, _model: &mut M) -> Result<()> {
        Ok(())
    }

    /// After a delete or force-delete was persisted.
    async fn deleted(&self, _target: &Target, _model: &M) -> Result<()> {
        Ok(())
    }

    /// Before a soft-delete restore. May adjust the instance.
    async fn restoring(&self, _target: &Target, _model: &mut M) -> Result<()> {
        Ok(())
    }

    /// After a restore was persisted.
    async fn restored(&self, _target: &Target, _model: &M) -> Result<()> {
        Ok(())
    }
}

/// Object-safe model handle for type-erased dispatch.
pub(crate) trait ErasedModel: Any + Send + Sync {
    /// Downcast support for the observer adapter (mutable hooks).
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Downcast support for the observer adapter (post-write hooks).
    fn as_any_ref(&self) -> &dyn Any;
}

impl<T: Model> ErasedModel for T {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

/// Pre-write hooks: receive `&mut`, may adjust the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreHook {
    Saving,
    Creating,
    Updating,
    Deleting,
    Restoring,
}

/// Post-write hooks: receive `&`; the row is already persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostHook {
    Saved,
    Created,
    Updated,
    Deleted,
    Restored,
}

/// Type-erased [`Observer`]: one registry serves every model.
///
/// Two methods (not ten): `macro_rules!` cannot generate `#[async_trait]`
/// methods (attribute macros expand before inner macros, so generated
/// methods would miss the transform), and ten hand-written forwarders add
/// nothing over a match.
#[async_trait]
pub(crate) trait ErasedObserver: Send + Sync {
    async fn fire_pre(
        &self,
        hook: PreHook,
        target: &Target,
        model: &mut dyn ErasedModel,
    ) -> Result<()>;
    async fn fire_post(
        &self,
        hook: PostHook,
        target: &Target,
        model: &dyn ErasedModel,
    ) -> Result<()>;
}

/// Adapts a typed observer to erased dispatch. The downcast cannot fail:
/// entries are keyed by `TypeId` and only dispatched for their own model —
/// the error arm is unreachable defense, never a silent skip.
struct Adapter<O, M> {
    observer: O,
    model: PhantomData<M>,
}

#[async_trait]
impl<O, M> ErasedObserver for Adapter<O, M>
where
    O: Observer<M>,
    M: Model,
{
    async fn fire_pre(
        &self,
        hook: PreHook,
        target: &Target,
        model: &mut dyn ErasedModel,
    ) -> Result<()> {
        let concrete = model
            .as_any_mut()
            .downcast_mut::<M>()
            .ok_or_else(|| Error::internal("observer registry type mismatch"))?;
        match hook {
            PreHook::Saving => self.observer.saving(target, concrete).await,
            PreHook::Creating => self.observer.creating(target, concrete).await,
            PreHook::Updating => self.observer.updating(target, concrete).await,
            PreHook::Deleting => self.observer.deleting(target, concrete).await,
            PreHook::Restoring => self.observer.restoring(target, concrete).await,
        }
    }

    async fn fire_post(
        &self,
        hook: PostHook,
        target: &Target,
        model: &dyn ErasedModel,
    ) -> Result<()> {
        // `ErasedModel: Any` gives `downcast_ref` through the supertrait.
        let concrete = model
            .as_any_ref()
            .downcast_ref::<M>()
            .ok_or_else(|| Error::internal("observer registry type mismatch"))?;
        match hook {
            PostHook::Saved => self.observer.saved(target, concrete).await,
            PostHook::Created => self.observer.created(target, concrete).await,
            PostHook::Updated => self.observer.updated(target, concrete).await,
            PostHook::Deleted => self.observer.deleted(target, concrete).await,
            PostHook::Restored => self.observer.restored(target, concrete).await,
        }
    }
}

/// Per-model observer lists, keyed by model `TypeId`.
#[derive(Default)]
pub(crate) struct ObserverRegistry {
    hooks: HashMap<TypeId, Vec<Arc<dyn ErasedObserver>>>,
}

impl ObserverRegistry {
    /// Creates an empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers an observer for model `M` (multiple observers per model run
    /// in registration order).
    pub(crate) fn add<M: Model, O: Observer<M> + 'static>(&mut self, observer: O) {
        self.hooks
            .entry(TypeId::of::<M>())
            .or_default()
            .push(Arc::new(Adapter {
                observer,
                model: PhantomData::<M>,
            }));
    }

    /// Clones the observer list for `M` (empty when none registered).
    fn cloned_for<M: Model>(&self) -> Vec<Arc<dyn ErasedObserver>> {
        self.hooks
            .get(&TypeId::of::<M>())
            .cloned()
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for ObserverRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObserverRegistry")
            .field("models", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

/// Dispatches one pre-write hook (`&mut`) to every observer of `M`.
/// The registry lock is released before any hook runs.
async fn fire_mut<M: Model>(target: &Target, hook: PreHook, model: &mut M) -> Result<()> {
    let observers = target
        .db()
        .observers()
        .read()
        .map(|registry| registry.cloned_for::<M>())
        .map_err(|_| Error::internal("observer registry lock poisoned"))?;
    for observer in observers {
        observer.fire_pre(hook, target, model).await?;
    }
    Ok(())
}

/// Dispatches one post-write hook (`&`) to every observer of `M`.
async fn fire_ref<M: Model>(target: &Target, hook: PostHook, model: &M) -> Result<()> {
    let observers = target
        .db()
        .observers()
        .read()
        .map(|registry| registry.cloned_for::<M>())
        .map_err(|_| Error::internal("observer registry lock poisoned"))?;
    for observer in observers {
        observer.fire_post(hook, target, model).await?;
    }
    Ok(())
}

macro_rules! fire_mut_hook {
    ($fire:ident, $hook:ident) => {
        pub(crate) async fn $fire<M: Model>(target: &Target, model: &mut M) -> Result<()> {
            fire_mut(target, PreHook::$hook, model).await
        }
    };
}

macro_rules! fire_ref_hook {
    ($fire:ident, $hook:ident) => {
        pub(crate) async fn $fire<M: Model>(target: &Target, model: &M) -> Result<()> {
            fire_ref(target, PostHook::$hook, model).await
        }
    };
}

fire_mut_hook!(fire_saving, Saving);
fire_ref_hook!(fire_saved, Saved);
fire_mut_hook!(fire_creating, Creating);
fire_ref_hook!(fire_created, Created);
fire_mut_hook!(fire_updating, Updating);
fire_ref_hook!(fire_updated, Updated);
fire_mut_hook!(fire_deleting, Deleting);
fire_ref_hook!(fire_deleted, Deleted);
fire_mut_hook!(fire_restoring, Restoring);
fire_ref_hook!(fire_restored, Restored);
