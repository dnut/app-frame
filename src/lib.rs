//! App Frame is a compile-time dependency-injected application framework with a
//! service orchestrator.
//!
//! At compile-time, the framework guarantees that all necessary dependencies
//! will be injected upon application startup. You can define dependency
//! relationships using the provided macros or with custom implementations. At
//! runtime, the framework triggers initialization code, then runs services,
//! monitors their health, and reports that health to any external http health
//! checks.
//!
//! This trivial example illustrates the bare minimum boilerplate to use the
//! framework, but doesn't actually run anything useful.
//!
//! ```rust
//! use app_frame::{application, service_manager::Application, Never};
//!
//! async fn tokio_main() -> anyhow::Result<Never> {
//!     MyApp.run(3600).await
//! }
//!
//! pub struct MyApp;
//!
//! application!(self: MyApp);
//! ```
//!
//! This example defines and injects various types of components to illustrate
//! the various features provided by the framework:
//!
//! ```rust
//! use std::sync::Arc;
//!
//! use async_trait::async_trait;
//!
//! use app_frame::{
//!     application,
//!     dependency_injection::Provides,
//!     inject,
//!     service::{Job, LoopConfig, LoopingJobService, SelfConfiguredLoop, Service},
//!     service_manager::{Heartbeat, Application},
//!     Never,
//! };
//!
//! async fn tokio_main() -> anyhow::Result<Never> {
//!     MyApp::new(()).run(3600).await
//! }
//!
//! pub struct MyApp {
//!     db_singleton: Arc<DatabaseConnectionPoolSingleton>,
//! }
//!
//! impl MyApp {
//!     pub fn new(database_config: ()) -> Self {
//!         Self {
//!             db_singleton: Arc::new(DatabaseConnectionPoolSingleton {
//!                 state: database_config,
//!             }),
//!         }
//!     }
//! }
//!
//! // Including a type here implements Provides<ThatType> for MyApp.
//! //
//! // Struct definitions wrapped in the `inject!` macro get a From<T>
//! // implementation where T: Provides<U> for each field of type U in the struct.
//! // When those structs are provided as a component here, they will be constructed
//! // with the assumption that MyApp impl Provides<U> for each of those U's
//! //
//! // All the types provided here are instantiated separately each time they are
//! // needed. If you want to support a singleton pattern, you need to construct the
//! // singletons in the constructor for this type and wrap them in an Arc. Then you
//! // can provide them in the "provided" section by cloning the Arc.
//! application! {
//!     self: MyApp
//!
//!     // init jobs are types implementing `Job` with a `run_once` function that
//!     // needs to run once during startup.
//!     // - constructed the same way as a component
//!     // - made available as a dependency, like a component
//!     // - wrap in curly braces for custom construction of an iterable of jobs.
//!     init [
//!         InitJob
//!     ]
//!
//!     // Services are types with a `run_forever` function that needs to run for
//!     // the entire lifetime of the application.
//!     // - constructed the same way as a component
//!     // - made available as a dependency, like a component
//!     // - registered as a service and spawned on startup.
//!     // - wrap in curly braces for custom construction of an iterable of
//!     //   services.
//!     // - Use 'as WrapperType' if it needs to be wrapped in order to get
//!     //   something that implements `Service`. wrapping uses WrapperType::from().
//!     services [
//!         MyService,
//!         JobToLoopForever as LoopingJobService,
//!     ]
//!
//!     // Components are items that will be provided as dependencies to anything
//!     // that needs it. This is similar to the types provided in the "provides"
//!     // section, except that components can be built exclusively from other
//!     // components and provided types, whereas "provides" items depend on other
//!     // state or logic.
//!     // - constructed via Type::from(MyApp). Use the inject! macro on the
//!     //   type to make this possible.
//!     // - Use `as dyn SomeTrait` if you also want to provide the type as the
//!     //   implementation for Arc<dyn SomeTrait>
//!     components [
//!         Component1,
//!         Component2,
//!         DatabaseRepository as dyn Repository,
//!     ]
//!
//!     // Use this when you want to provide a value of some type that needs to either:
//!     // - be constructed by some custom code you want to write here.
//!     // - depend on some state that was initialized in MyApp.
//!     //
//!     // Syntax: Provide a list of the types you want to provide, followed by the
//!     // expression that can be used to instantiate any of those types.
//!     // ```
//!     // TypeToProvide: { let x = self.get_x(); TypeToProvide::new(x) },
//!     // Arc<dyn Trait>, Arc<ConcreteType>: Arc::new(ConcreteType::default()),
//!     // ```
//!     provided {
//!         Arc<DatabaseConnectionPoolSingleton>: self.db_singleton.clone(),
//!     }
//! }
//!
//! inject!(
//!     pub struct InitJob {
//!         repo: Arc<dyn Repository>,
//!     }
//! );
//!
//! #[async_trait]
//! impl Job for InitJob {
//!     async fn run_once(&self) -> anyhow::Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! inject!(
//!     pub struct JobToLoopForever {
//!         c1: Component1,
//!         c2: Component2,
//!     }
//! );
//!
//! #[async_trait]
//! impl Job for JobToLoopForever {
//!     async fn run_once(&self) -> anyhow::Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! impl SelfConfiguredLoop for JobToLoopForever {
//!     fn loop_config(&self) -> LoopConfig {
//!         LoopConfig {
//!             delay_secs: 10,
//!             max_iteration_secs: 20,
//!         }
//!     }
//! }
//!
//! inject!(
//!     pub struct Component1 {}
//! );
//!
//! inject!(
//!     pub struct Component2 {
//!         repo: Arc<dyn Repository>,
//!     }
//! );
//!
//! pub trait Repository: Send + Sync {}
//!
//! pub struct MyService {
//!     repository: Arc<dyn Repository>,
//!     heartbeat: Arc<dyn Heartbeat + 'static>,
//!     my_health_metric: bool,
//! }
//!
//! /// This is how you provide a custom alternative to the `inject!` macro, it is
//! /// practical here since only one item needs to be injected, and the others can
//! /// be set to default values.
//! impl<T> From<&T> for MyService
//! where
//!     T: Provides<Arc<dyn Repository>>,
//! {
//!     fn from(p: &T) -> Self {
//!         Self {
//!             repository: p.provide(),
//!             heartbeat: Arc::new(()),
//!             my_health_metric: true,
//!         }
//!     }
//! }
//!
//! #[async_trait]
//! impl Service for MyService {
//!     async fn run_forever(&self) -> Never {
//!         loop {
//!             self.heartbeat.beat();
//!         }
//!     }
//!
//!     fn heartbeat_ttl(&self) -> i32 {
//!         60
//!     }
//!
//!     fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat + 'static>) {
//!         self.heartbeat = heartbeat;
//!     }
//!
//!     fn is_healthy(&self) -> bool {
//!         self.my_health_metric
//!     }
//! }
//!
//! inject!(
//!     pub struct DatabaseRepository {
//!         connection: Arc<DatabaseConnectionPoolSingleton>,
//!     }
//! );
//!
//! impl Repository for DatabaseRepository {}
//!
//! pub struct DatabaseConnectionPoolSingleton {
//!     state: (),
//! }
//! ```

/// Exponential backoff tracker to inform callers when to take an action.
pub mod backoff;
/// Define a type as a dependent or a dependency provider.
pub mod dependency_injection;
/// Simple and versatile error handling and logging.
pub mod error;
/// Externally facing http endpoint to report service health.
pub mod health_endpoint;
/// How to consume and process items from a queue.
pub mod queue_consumer;
/// Defines which behaviors are required to define a job or service.
pub mod service;
/// Runs services and monitors their health.
pub mod service_manager;
/// Clock dependencies that are easily swapped out and mocked, to reduce direct dependencies on syscalls.
pub mod time;

/// misc items that are too small to get their own files,
/// kept out of this file to reduce clutter.
mod util;
pub use util::*;
