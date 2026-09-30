//! Service container: explicit, instance-based dependency injection.
//!
//! The container holds singletons (one shared instance) and transient
//! bindings (a factory called per resolution). There is no global state and
//! no reflection: bindings are registered by concrete type in service
//! providers, and controllers resolve their fields via the generated
//! `from_container` constructor.
//!
//! Resolution returns [`Arc`] handles, so services never need to be `Clone`
//! (for `Clone` services, [`Container::resolve_value`] unwraps one step
//! further). Factories receive `&Container`, so bindings can compose.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{AppError, Result};

/// Type-erased factory: builds a service from the container on demand.
type Factory = Arc<dyn Fn(&Container) -> Arc<dyn Any + Send + Sync> + Send + Sync>;

/// Instance-based service container.
///
/// # Examples
///
/// ```rust
/// use lumos_core::Container;
/// use std::sync::Arc;
///
/// #[derive(Debug, PartialEq)]
/// struct Greeting(String);
///
/// let mut container = Container::new();
/// container.singleton_value(Greeting("hello".to_string()));
///
/// let greeting: Arc<Greeting> = container.resolve().unwrap();
/// assert_eq!(greeting.0, "hello");
/// ```
#[derive(Default)]
pub struct Container {
    bindings: HashMap<TypeId, Binding>,
}

enum Binding {
    Singleton(Arc<dyn Any + Send + Sync>),
    Factory(Factory),
}

impl Container {
    /// Creates an empty container.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    ///
    /// let container = Container::new();
    /// assert!(!container.has::<String>());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a shared instance, resolved by cloning the [`Arc`].
    ///
    /// Re-registering the same type replaces the previous binding.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    /// use std::sync::Arc;
    ///
    /// let mut container = Container::new();
    /// container.singleton(Arc::new(7u32));
    ///
    /// let first: Arc<u32> = container.resolve().unwrap();
    /// let second: Arc<u32> = container.resolve().unwrap();
    /// assert!(Arc::ptr_eq(&first, &second));
    /// ```
    pub fn singleton<T: Send + Sync + 'static>(&mut self, value: Arc<T>) -> &mut Self {
        self.bindings.insert(
            TypeId::of::<T>(),
            Binding::Singleton(value as Arc<dyn Any + Send + Sync>),
        );
        self
    }

    /// Registers a shared instance by value (wrapped in an [`Arc`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    ///
    /// let mut container = Container::new();
    /// container.singleton_value(vec![1, 2, 3]);
    ///
    /// let values: Vec<i32> = container.resolve_value().unwrap();
    /// assert_eq!(values, vec![1, 2, 3]);
    /// ```
    pub fn singleton_value<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.singleton(Arc::new(value))
    }

    /// Registers a transient binding: `factory` runs on every [`resolve`](Container::resolve).
    ///
    /// The factory receives the container, so bindings can resolve other
    /// bindings. Re-registering the same type replaces the previous binding.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    /// use std::sync::Arc;
    /// use std::sync::atomic::{AtomicU32, Ordering};
    ///
    /// let counter = Arc::new(AtomicU32::new(0));
    /// let mut container = Container::new();
    /// container.bind({
    ///     let counter = Arc::clone(&counter);
    ///     move |_| Arc::new(counter.fetch_add(1, Ordering::SeqCst))
    /// });
    ///
    /// let first: Arc<u32> = container.resolve().unwrap();
    /// let second: Arc<u32> = container.resolve().unwrap();
    /// assert_ne!(first, second);
    /// ```
    pub fn bind<T, F>(&mut self, factory: F) -> &mut Self
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> Arc<T> + Send + Sync + 'static,
    {
        let erased: Factory = Arc::new(move |container| {
            let value: Arc<T> = factory(container);
            value as Arc<dyn Any + Send + Sync>
        });
        self.bindings.insert(TypeId::of::<T>(), Binding::Factory(erased));
        self
    }

    /// Registers a transient binding that produces values by clone.
    ///
    /// Equivalent to [`bind`](Container::bind) with the factory output
    /// wrapped in an [`Arc`]; resolution still returns shared handles.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    ///
    /// let mut container = Container::new();
    /// container.bind_value(|_| "fresh".to_string());
    ///
    /// let value: String = container.resolve_value().unwrap();
    /// assert_eq!(value, "fresh");
    /// ```
    pub fn bind_value<T, F>(&mut self, factory: F) -> &mut Self
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> T + Send + Sync + 'static,
    {
        self.bind(move |container| Arc::new(factory(container)))
    }

    /// Resolves a shared handle to `T`.
    ///
    /// Singletons win over transient bindings when both exist (unusual, but
    /// deterministic). Resolving an unregistered type is an
    /// [`AppError::Internal`] carrying the missing type name for operators.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    /// use std::sync::Arc;
    ///
    /// let mut container = Container::new();
    /// container.singleton_value(1u8);
    ///
    /// let one: Arc<u8> = container.resolve().unwrap();
    /// assert_eq!(*one, 1);
    /// assert!(container.resolve::<u16>().is_err());
    /// ```
    pub fn resolve<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        let id = TypeId::of::<T>();
        if let Some(binding) = self.bindings.get(&id) {
            return match binding {
                Binding::Singleton(stored) => downcast::<T>(Arc::clone(stored)),
                Binding::Factory(factory) => downcast::<T>(factory(self)),
            };
        }
        Err(AppError::internal(format!(
            "no binding registered for {}",
            std::any::type_name::<T>()
        )))
    }

    /// Resolves `T` by value (for `Clone` services).
    ///
    /// This is what generated controller constructors use: each field type
    /// must be `Clone` (or the controller holds `Arc<T>` fields resolved
    /// via [`resolve`](Container::resolve) instead).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    ///
    /// let mut container = Container::new();
    /// container.singleton_value("owned".to_string());
    ///
    /// let value: String = container.resolve_value().unwrap();
    /// assert_eq!(value, "owned");
    /// ```
    pub fn resolve_value<T: Clone + Send + Sync + 'static>(&self) -> Result<T> {
        self.resolve::<T>().map(|shared| (*shared).clone())
    }

    /// Returns `true` when `T` has a singleton or transient binding.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Container;
    ///
    /// let mut container = Container::new();
    /// assert!(!container.has::<u32>());
    /// container.singleton_value(1u32);
    /// assert!(container.has::<u32>());
    /// ```
    pub fn has<T: 'static>(&self) -> bool {
        let id = TypeId::of::<T>();
        self.bindings.contains_key(&id)
    }
}

impl std::fmt::Debug for Container {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Container")
            .field("bindings", &self.bindings.len())
            .finish()
    }
}

/// Downcasts a type-erased handle. The mismatch arm is defensive (keys are
/// always `TypeId::of::<T>`), but stays explicit rather than unwrapping.
fn downcast<T: Send + Sync + 'static>(erased: Arc<dyn Any + Send + Sync>) -> Result<Arc<T>> {
    erased.downcast::<T>().map_err(|_| {
        AppError::internal(format!(
            "container type mismatch for {}",
            std::any::type_name::<T>()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_resolves_the_same_allocation() {
        let mut container = Container::new();
        container.singleton_value(String::from("shared"));
        let first: Arc<String> = container.resolve().unwrap();
        let second: Arc<String> = container.resolve().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn factories_compose_through_the_container() {
        let mut container = Container::new();
        container.singleton_value(21u32);
        container.bind_value(|container| u64::from(container.resolve_value::<u32>().unwrap()) * 2);
        let doubled: u64 = container.resolve_value().unwrap();
        assert_eq!(doubled, 42);
    }

    #[test]
    fn last_registration_replaces_the_previous_binding() {
        let mut container = Container::new();
        container.singleton_value(1u32);
        container.bind_value(|_| 2u32);
        let value: u32 = container.resolve_value().unwrap();
        assert_eq!(value, 2);

        container.singleton_value(3u32);
        let value: u32 = container.resolve_value().unwrap();
        assert_eq!(value, 3);
    }

    #[test]
    fn missing_binding_names_the_type() {
        let container = Container::new();
        let error = container.resolve::<Vec<u8>>().unwrap_err();
        assert!(error.to_string().contains("Vec<u8>"));
    }

    #[test]
    fn debug_summarizes_counts() {
        let mut container = Container::new();
        container.singleton_value(1u8);
        let debug = format!("{container:?}");
        assert!(debug.contains("bindings: 1"));
    }
}
