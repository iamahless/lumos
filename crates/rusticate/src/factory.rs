//! Factories and seeders: generated fixtures and database seeding.
//!
//! [`factory!`] defines a fixture builder for a model: a `definition(seq)`
//! function producing a [`Changeset`](crate::Changeset), with sequence
//! numbers making every instance unique (`user1@example.com`,
//! `user2@example.com`, …). No fake-data dependency — sequences plus explicit
//! values cover the deterministic cases; random data comes from the caller's
//! own generator inside `definition` when needed.
//!
//! [`Seeder`] is the run-once counterpart: implement it for a seed operation
//! (usually a few `factory.create()` calls) and invoke it from a migration,
//! a CLI command, or test setup.

use async_trait::async_trait;

use crate::db::DB;
use crate::Result;

/// Defines a fixture factory for a model.
///
/// # Examples
///
/// ```rust
/// use rusticate::{factory, Changeset, Model};
///
/// # #[derive(Model)]
/// # #[model(table = "users")]
/// # pub struct User {
/// #     #[model(id, auto_increment)]
/// #     pub id: i64,
/// #     pub name: String,
/// #     pub email: String,
/// # }
///
/// factory!(UserFactory, User, |seq| {
///     Changeset::new()
///         .set("name", format!("User {seq}"))
///         .set("email", format!("user{seq}@example.com"))
/// });
/// ```
///
/// The generated struct holds an atomic sequence counter (starting at 1) and
/// offers:
///
/// - `new()` / `Default`: builds the factory.
/// - `make(&self) -> Changeset`: runs `definition` with the next sequence number.
/// - `make_with(&self, overrides: Changeset) -> Changeset`: `make`, with
///   `overrides` winning per column.
/// - `create(&self, target) -> Result<Model>`: `make` + [`Model::create`](crate::Model::create).
/// - `create_with(&self, target, overrides) -> Result<Model>`: `make_with` + create.
/// - `create_many(&self, target, count) -> Result<Vec<Model>>`: `count` creates.
///
/// Factories are `Sync`, so one instance serves concurrent seeds; sequence
/// numbers stay unique but unordered under contention.
#[macro_export]
macro_rules! factory {
    ($name:ident, $model:ty, |$seq:ident| $body:block) => {
        #[doc = "Fixture factory (see `rusticate::factory!`)."]
        pub struct $name {
            seq: ::std::sync::atomic::AtomicU64,
        }

        impl $name {
            #[doc = "Builds the factory (sequence starts at 1)."]
            pub fn new() -> Self {
                Self {
                    seq: ::std::sync::atomic::AtomicU64::new(1),
                }
            }

            #[doc = "Runs `definition` with the next sequence number."]
            pub fn make(&self) -> $crate::Changeset {
                let $seq = self
                    .seq
                    .fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
                $body
            }

            #[doc = "`make`, with `overrides` winning per column."]
            pub fn make_with(&self, overrides: $crate::Changeset) -> $crate::Changeset {
                let mut base = self.make();
                for (column, value) in overrides.iter() {
                    base.put(column, value.clone());
                }
                base
            }

            #[doc = "`make` + insert; returns the persisted instance."]
            pub async fn create(
                &self,
                target: impl Into<$crate::Target>,
            ) -> $crate::Result<$model> {
                let changeset = self.make();
                <$model as $crate::Model>::create(target, changeset).await
            }

            #[doc = "`make_with` + insert; returns the persisted instance."]
            pub async fn create_with(
                &self,
                target: impl Into<$crate::Target>,
                overrides: $crate::Changeset,
            ) -> $crate::Result<$model> {
                let changeset = self.make_with(overrides);
                <$model as $crate::Model>::create(target, changeset).await
            }

            #[doc = "Inserts `count` instances (sequence numbers stay unique)."]
            pub async fn create_many(
                &self,
                target: impl Into<$crate::Target>,
                count: usize,
            ) -> $crate::Result<Vec<$model>> {
                let target = target.into();
                let mut models = Vec::with_capacity(count);
                for _ in 0..count {
                    models.push(self.create(target.clone()).await?);
                }
                Ok(models)
            }
        }

        impl ::std::default::Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl ::std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter
                    .debug_struct(::std::stringify!($name))
                    .field(
                        "seq",
                        &self.seq.load(::std::sync::atomic::Ordering::Relaxed),
                    )
                    .finish()
            }
        }
    };
}

/// A run-once seed operation, usually a few factory `create` calls.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{async_trait, Changeset, Model, Result, Seeder, DB};
/// # #[derive(Model)]
/// # #[model(table = "users")]
/// # pub struct User {
/// #     #[model(id, auto_increment)]
/// #     pub id: i64,
/// #     pub email: String,
/// # }
///
/// pub struct AdminSeeder;
///
/// #[async_trait]
/// impl Seeder for AdminSeeder {
///     async fn run(&self, db: &DB) -> Result<()> {
///         User::create(db, Changeset::new().set("email", "admin@example.com")).await?;
///         Ok(())
///     }
/// }
/// ```
///
/// Invoke seeders explicitly — from a migration's `up`, a CLI command, or
/// test setup. For all-or-nothing seeding, call `db.transaction` inside
/// `run` and target the transaction.
#[async_trait]
pub trait Seeder: Send + Sync {
    /// Stable record name (defaults to the type path; override for prettier
    /// `db:seed` output and `--seeder` filtering).
    fn name(&self) -> String {
        std::any::type_name::<Self>().to_string()
    }

    /// Runs the seed operation against `db`.
    async fn run(&self, db: &DB) -> Result<()>;
}
