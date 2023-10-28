pub trait Provides<T> {
    fn provide(&self) -> T;
}

impl<T: Clone> Provides<T> for T {
    fn provide(&self) -> T {
        self.clone()
    }
}

pub trait ProvideA {
    fn provide_a<T>(&self) -> T
    where
        Self: Provides<T>,
    {
        self.provide()
    }
}
impl<T> ProvideA for T {}

/// Syntactic sugar to make a type's dependencies injectable.
///
/// ```rust ignore
/// inject!(
///     pub struct MyStruct {
///         some_component: SomeComponent,
///         some_behavior: Arc<dyn SomeBehavior>,
///     }
/// );
/// ```
///
/// Under the hood, this macro implements From<&T> where T: Provides<FieldType>
/// for every field in the struct. If you'd like it to be constructed in a
/// different way, you can manually implement the trait instead of using the
/// macro.
#[macro_export]
macro_rules! inject {
    (
        $(#[$outer:meta])*
        pub struct $Name:ident {
            $($viz:vis $field:ident: $FieldType:ty),*$(,)?
        }
    ) => {
        $(#[$outer])*
        pub struct $Name {
            $($viz $field: $FieldType),*
        }
        impl<T> From<&T> for $Name where
            $(T: $crate::dependency_injection::Provides<$FieldType>),*
        {
            fn from(value: &T) -> Self {
                Self { $($field: value.provide()),* }
            }
        }
    };
}

/// Including a type here implements Provides<ThatType> for MyApp.
///
/// Struct definitions wrapped in the `inject!` macro get a From<T>
/// implementation where T: Provides<U> for each field of type U in the struct.
/// When those structs are provided as a component here, they will be
/// constructed with the assumption that MyApp impl Provides<U> for each of
/// those U's
///
/// All the types provided here are instantiated separately each time they are
/// needed. If you want to support a singleton pattern, you need to construct
/// the singletons in the constructor for this type and wrap them in an Arc.
/// Then you can provide them in the "provided" section by cloning the Arc.
///
/// Open the main readme to see the following example in context.
///
/// ```rust ignore
/// application! {
///     self: MyApp
///
///     // init jobs are types implementing `Job` with a `run_once` function that
///     // needs to run once during startup.
///     // - constructed the same way as a component
///     // - made available as a dependency, like a component
///     // - wrap in curly braces for custom construction of an iterable of jobs.
///     init [
///         InitJob
///     ]
///
///     // Services are types with a `run_forever` function that needs to run for
///     // the entire lifetime of the application.
///     // - constructed the same way as a component
///     // - made available as a dependency, like a component
///     // - registered as a service and spawned on startup.
///     // - wrap in curly braces for custom construction of an iterable of
///     //   services.
///     // - Use 'as WrapperType' if it needs to be wrapped in order to get
///     //   something that implements `Service`. wrapping uses WrapperType::from().
///     services [
///         MyService,
///         JobToLoopForever as LoopingJobService,
///     ]
///
///     // Components are items that will be provided as dependencies to anything
///     // that needs it. This is similar to the types provided in the "provides"
///     // section, except that components can be built exclusively from other
///     // components and provided types, whereas "provides" items depend on other
///     // state or logic.
///     // - constructed via Type::from(MyApp). Use the inject! macro on the
///     //   type to make this possible.
///     // - Use `as dyn SomeTrait` if you also want to provide the type as the
///     //   implementation for Arc<dyn SomeTrait>
///     components [
///         Component1,
///         Component2,
///         DatabaseRepository as dyn Repository,
///     ]
///
///     // Use this when you want to provide a value of some type that needs to either:
///     // - be constructed by some custom code you want to write here.
///     // - depend on some state that was initialized in MyApp.
///     //
///     // Syntax: Provide a list of the types you want to provide, followed by the
///     // expression that can be used to instantiate any of those types.
///     // ```
///     // TypeToProvide: { let x = self.get_x(); TypeToProvide::new(x) },
///     // Arc<dyn Trait>, Arc<ConcreteType>: Arc::new(ConcreteType::default()),
///     // ```
///     provided {
///         Arc<DatabaseConnectionPoolSingleton>: self.db_singleton.clone(),
///     }
/// }
/// ```
#[macro_export]
macro_rules! application {
    (
        $self:ident: $Provider:ident
        $(init [
            $($JobIterable:block,)*
            $($Job:ty $(as $JobAs:ty)?),*$(,)?
        ])?
        $(services [
            $($SvcIterable:block,)*
            $($Svc:ty $(as $SvcAs:ty)?),*$(,)?
        ])?
        $(components [
            $($Component:ty $(as $($CompAs:ty)|+)?),+$(,)?
        ])?
        $(provided {
            $($($Provided:ty),+: $logic:expr),+$(,)?
        })?
    ) => {
        // Init
        impl $crate::service_manager::Initialize for $Provider {
            fn init(&$self) -> Vec<std::sync::Arc<dyn $crate::service::Job>> {
                #[allow(unused_imports)]
                use $crate::dependency_injection::Provides;
                #[allow(unused_mut)]
                let mut jobs: Vec<std::sync::Arc<dyn $crate::service::Job>> = vec![];
                $(
                    $(for provided in $JobIterable {
                        jobs.push(provided);
                    })*
                    $(
                        let job = <$Job>::from($self);
                        $(let job = <$JobAs>::from(job);)?
                        jobs.push(std::sync::Arc::new(job));
                    )*
                )?
                jobs
            }
        }

        // Services
        impl $crate::service_manager::Serves for $Provider {
            fn services(&$self) -> Vec<Box<dyn $crate::service::Service>> {
                #[allow(unused_imports)]
                use $crate::dependency_injection::Provides;
                #[allow(unused_mut)]
                let mut services: Vec<Box<dyn $crate::service::Service>> = vec![];
                $(
                    $(for provided in $SvcIterable {
                        services.push(Box::new(provided));
                    })*
                    $(
                        let service = <$Svc>::from($self);
                        $(let service = <$SvcAs>::from(service);)?
                        services.push(Box::new(service));
                    )*
                )?
                services
            }
        }

        // Components
        $($(
            impl $crate::dependency_injection::Provides<$Component> for $Provider {
                fn provide(&self) -> $Component {
                    <$Component>::from(self)
                }
            }
            $(
                $(
                    impl $crate::dependency_injection::Provides<std::sync::Arc<$CompAs>> for $Provider {
                        fn provide(&self) -> std::sync::Arc<$CompAs> {
                            std::sync::Arc::new(<$Component>::from(self))
                        }
                    }
                )+
            )?
        )*)?
        // Provided
        $($($(
            impl $crate::dependency_injection::Provides<$Provided> for $Provider {
                fn provide(&$self) -> $Provided {
                    $logic
                }
            }
        )*)*)?
    }
}
