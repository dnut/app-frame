# App Frame

[github](https://github.com/dnut/app-frame)
· [crates.io](https://crates.io/crates/app-frame)
· [docs.rs](https://docs.rs/app-frame)

App Frame is a compile-time dependency-injected application framework with a service orchestrator. It has two general goals:
1. Reliably run and monitor multiple long-running services, recovering from any errors.
2. Reduce the boilerplate and maintenance cost of manually constructing application components that have a complex dependency graph. You should only need to describe your application's components. The rust compiler can wire them up.

At compile-time, the framework guarantees that all necessary dependencies will be injected upon application startup. At runtime, the framework runs your custom initialization code, then starts your services, automatically injects their dependencies, monitors their health, and reports their status to external http health checks.

Application frameworks add complexity and obscure control flow, but they can also save a lot of time with setup and maintenance tasks. To be sure app-frame is right for your project, see the [Trade-offs](#trade-offs) section.

## Usage

```toml
[dependencies]
app_frame = "0.2.1"
```

App Frame provides macros for convenience. If they feel too esoteric or inflexible, you can alternatively implement traits to wire up your application.

This trivial example illustrates the bare minimum boilerplate to use the framework with its macros, but doesn't actually run anything useful.

```rust
use app_frame::{application, service_manager::Application, Never};

#[tokio::main]
async fn main() -> anyhow::Result<Never> {
    MyApp.run().await
}

pub struct MyApp;

application!(self: MyApp);
```

Here is the equivalent application, with direct trait implementation instead of macros:

```rust
use app_frame::{application, service_manager::Application, Never};

#[tokio::main]
async fn main() -> anyhow::Result<Never> {
    MyApp.run().await
}

pub struct MyApp;

impl Initialize for MyApp {
    fn init(&self) -> Vec<Arc<dyn Job>> {
        vec![]
    }
}

impl Serves for MyApp {
    fn services(&self) -> Vec<Box<dyn Service>> {
        vec![]
    }
}
```


To take full advantage of app-frame, you should define dependency relationships and explicitly declare all the components that you want to run as part of your application.

### Make Components Injectible

Make a struct injectible if you want its dependencies to be injected by app-frame, with either a macro or trait:
- **macro:** Wrap the struct with the `inject!` macro. In a future release, this will be an attribute-style macro.
- **traits:** `impl<T> From<&T> for C where T: Provides<D>` for the component `C` and each of its dependencies `D`.
```rust
/// Macro approach. This automatically implements:
/// impl<T> From<&T> for InitJob 
/// where 
///       T: Provides<Arc<dyn Repository>> + Provides<Component2> {...}
inject!(
    pub struct Component1 {
        repo: Arc<dyn Repository>,
        component2: Component2,
    }
);
```

```rust
/// Trait approach
/// 
/// This is practical here since only one item needs to be injected,
/// and the others can be set to default values. The inject macro
/// does not yet support defaults.
impl<T> From<&T> for MyService
where
    T: Provides<Arc<dyn Repository>>,
{
    fn from(p: &T) -> Self {
        Self {
            repository: p.provide(),
            heartbeat: Arc::new(()),
            my_health_metric: true,
        }
    }
}
```

### Define Services
App Frame is built with the assumption that it will trigger some long running services on startup. To define a long-running service, either implement `Service`, or implement `Job` and `SelfConfiguredLoop`.
```rust
/// Implement Service when you already have some logic that runs forever
#[async_trait]
impl Service for MyService {
    async fn run_forever(&self) -> Never {
        // This is a trivial example. You can call something that you expect to run forever.
        loop {
            self.heartbeat.beat();
        }
    }

    fn heartbeat_ttl(&self) -> i32 {
        60
    }

    fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat + 'static>) {
        self.heartbeat = heartbeat;
    }

    fn is_healthy(&self) -> bool {
        self.my_health_metric
    }
}
```

```rust
/// Implementing these traits will let you use a job that is expected to terminate
/// after a short time. It becomes a service that runs the job repeatedly.
#[async_trait]
impl Job for JobToLoopForever {
    async fn run_once(&self) -> anyhow::Result<()> {
        self.solve_halting_problem()
    }
}

impl SelfConfiguredLoop for JobToLoopForever {
    fn loop_config(&self) -> LoopConfig {
        LoopConfig {
            delay_secs: 10,
            max_iteration_secs: 20,
        }
    }
}
```


### Declare the app
Define a struct representing your app and populate it with any configuration or singletons.

Any components that are automatically constructed by the framework will be instantiated once for every component that depends on it. If you want a singleton that is created once and reused, it needs to be instantiated manually, like this example.
```rust
pub struct MyApp {
    db_singleton: Arc<DatabaseConnectionPoolSingleton>,
}

impl MyApp {
    pub fn new(database_config: &str) -> Self {
        Self {
            db_singleton: Arc::new(DatabaseConnectionPoolSingleton {
                conn: database_config.into(),
            }),
        }
    }
}
```

### Declare application components.
Many dependency injection frameworks only need you to define your components, and the framework will locate them and create them as needed. App Frame instead prefers explicitness. You must list all application components in a single place, but App Frame will figure out how to wire them together. This is intended to make the framework's control flow easier to follow, though this benefit may be offset by using macros.

- **macro:** List all of your app's components in the `application!` macro as documented below.
- **traits:**
  - `impl Initialize for MyApp`, providing any `Job`s that need to run at startup.
  - `impl Serves for MyApp`, providing any `Service`s that need to run continuously.
  - `impl Provides<D> for MyApp` for each dependency `D` that will be needed either directly or transitively by any job or service provided above.


### Customize Service Orchestration

You can customize monitoring and recovery behavior by starting your app with the `run_custom` method.

```rust
// This is the default config, which is used when you call `MyApp::new().run()`
// See rustdocs for RunConfig and HealthEndpointConfig for more details.
MyApp::new()
    .run_custom(RunConfig {
        log_interval: 21600,
        attempt_recovery_after: 120,
        http_health_endpoint: Some(HealthEndpointConfig {
            port: 3417,
            success_status: StatusCode::OK,
            fail_status: StatusCode::INTERNAL_SERVER_ERROR,
        }),
    })
    .await
```

### Full macro-based example

This example defines and injects various types of components to illustrate the various features provided by the framework. This code actually runs an application, and will respond to health checks indicating that 2 services are healthy.

```rust
use std::sync::Arc;

use async_trait::async_trait;

use app_frame::{
    application,
    dependency_injection::Provides,
    inject,
    service::{Job, LoopConfig, LoopingJobService, SelfConfiguredLoop, Service},
    service_manager::{Application, Heartbeat},
    Never,
};

#[tokio::main]
async fn main() -> anyhow::Result<Never> {
    MyApp::new("db://host").run().await
}

pub struct MyApp {
    db_singleton: Arc<DatabaseConnectionPoolSingleton>,
}

impl MyApp {
    pub fn new(database_config: &str) -> Self {
        Self {
            db_singleton: Arc::new(DatabaseConnectionPoolSingleton {
                conn: database_config.into(),
            }),
        }
    }
}

// Including a type here implements Provides<ThatType> for MyApp.
//
// Struct definitions wrapped in the `inject!` macro get a From<T>
// implementation where T: Provides<U> for each field of type U in the struct.
// When those structs are provided as a component here, they will be constructed
// with the assumption that MyApp impl Provides<U> for each of those U's
//
// All the types provided here are instantiated separately each time they are
// needed. If you want to support a singleton pattern, you need to construct the
// singletons in the constructor for this type and wrap them in an Arc. Then you
// can provide them in the "provided" section by cloning the Arc.
application! {
    self: MyApp

    // init jobs are types implementing `Job` with a `run_once` function that
    // needs to run once during startup.
    // - constructed the same way as a component
    // - made available as a dependency, like a component
    // - wrap in curly braces for custom construction of an iterable of jobs.
    init [
        InitJob
    ]

    // Services are types with a `run_forever` function that needs to run for
    // the entire lifetime of the application.
    // - constructed the same way as a component
    // - made available as a dependency, like a component
    // - registered as a service and spawned on startup.
    // - wrap in curly braces for custom construction of an iterable of
    //   services.
    // - Use 'as WrapperType' if it needs to be wrapped in order to get
    //   something that implements `Service`. wrapping uses WrapperType::from().
    services [
        MyService,
        JobToLoopForever as LoopingJobService,
    ]

    // Components are items that will be provided as dependencies to anything
    // that needs it. This is similar to the types provided in the "provides"
    // section, except that components can be built exclusively from other
    // components and provided types, whereas "provides" items depend on other
    // state or logic.
    // - constructed via Type::from(MyApp). Use the inject! macro on the
    //   type to make this possible.
    // - Use `as dyn SomeTrait` if you also want to provide the type as the
    //   implementation for Arc<dyn SomeTrait>
    components [
        Component1,
        Component2,
        DatabaseRepository as dyn Repository,
    ]

    // Use this when you want to provide a value of some type that needs to either:
    // - be constructed by some custom code you want to write here.
    // - depend on some state that was initialized in MyApp.
    //
    // Syntax: Provide a list of the types you want to provide, followed by the
    // expression that can be used to instantiate any of those types.
    // ```
    // TypeToProvide: { let x = self.get_x(); TypeToProvide::new(x) },
    // Arc<dyn Trait>, Arc<ConcreteType>: Arc::new(ConcreteType::default()),
    // ```
    provided {
        Arc<DatabaseConnectionPoolSingleton>: self.db_singleton.clone(),
    }
}

inject!(
    pub struct InitJob {
        repo: Arc<dyn Repository>,
    }
);

#[async_trait]
impl Job for InitJob {
    async fn run_once(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

inject!(
    pub struct JobToLoopForever {
        c1: Component1,
        c2: Component2,
    }
);

#[async_trait]
impl Job for JobToLoopForever {
    async fn run_once(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl SelfConfiguredLoop for JobToLoopForever {
    fn loop_config(&self) -> LoopConfig {
        LoopConfig {
            delay_secs: 10,
            max_iteration_secs: 20,
        }
    }
}

inject!(
    pub struct Component1 {}
);

inject!(
    pub struct Component2 {
        repo: Arc<dyn Repository>,
    }
);

pub trait Repository: Send + Sync {}

pub struct MyService {
    repository: Arc<dyn Repository>,
    heartbeat: Arc<dyn Heartbeat + 'static>,
    my_health_metric: bool,
}

/// This is how you provide a custom alternative to the `inject!` macro, it is
/// practical here since only one item needs to be injected, and the others can
/// be set to default values.
impl<T> From<&T> for MyService
where
    T: Provides<Arc<dyn Repository>>,
{
    fn from(p: &T) -> Self {
        Self {
            repository: p.provide(),
            heartbeat: Arc::new(()),
            my_health_metric: true,
        }
    }
}

#[async_trait]
impl Service for MyService {
    async fn run_forever(&self) -> Never {
        loop {
            self.heartbeat.beat();
        }
    }

    fn heartbeat_ttl(&self) -> i32 {
        60
    }

    fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat + 'static>) {
        self.heartbeat = heartbeat;
    }

    fn is_healthy(&self) -> bool {
        self.my_health_metric
    }
}

inject!(
    pub struct DatabaseRepository {
        connection: Arc<DatabaseConnectionPoolSingleton>,
    }
);

impl Repository for DatabaseRepository {}

pub struct DatabaseConnectionPoolSingleton {
    conn: String,
}
```

# Trade-offs
Application frameworks are often not worth the complexity, but they do have utility in many cases. App Frame would typically be useful in a complicated backend web service with a lot of connections to other services, or whenever the following conditions are met:

- Multiple fallible long running tasks need to run indefinitely in parallel with monitoring and recovery.
- You want to use tokio's event loop to run suspending functions in parallel.
- The decoupling achieved by dependency inversion is beneficial enough to be worth the complexity of introducing layers of abstraction.
- The application has a complex dependency graph of internal components, and you'd like to make it easier to change the dependency relationships in the future.
- You want to be explicit, in a single place, about every component that should actually be instantiated when the app starts. This is a key differentiator from most other dependency injection frameworks.
- You don't mind gigantic macros that only make sense after reading documentation. Macros are not required to use App Frame, but you can use them to significantly reduce boilerplate.
- You are willing to compromise some performance and readability with the indirection of vtables and smart pointers.