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

/// Implement constructors that can be used for dependency injection:
/// - `new` method for each field
/// - impl From<&T> where T: Provide<FieldType> for every field
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

#[macro_export]
macro_rules! application {
    (
        $self:ident: $Provider:ident
        $(init [
            $($JobIterable:block $(as [$ProvidedJobType:ident])?,)*
            $($Job:ty $(as $($JobAs:ty)|+)?),*$(,)?
        ])?
        $(services [
            $($SvcIterable:block $(as [$ProvidedSvcType:ident])?,)*
            $($Svc:ty $(as $($SvcAs:ty)|+)?),*$(,)?
        ])?
        $(components [
            // $($Component:ty $(: $ComponentType:ident)?),+$(,)?
            $($Component:ty $(as $($CompAs:ty)|+)?),+$(,)?
        ])?
        $(provided {
            $($($Provided:ty),+: $logic:expr),+$(,)?
        })?
    ) => {
        // Init
        $(
            $(
                impl $crate::dependency_injection::Provides<$Job> for $Provider {
                    fn provide(&self) -> $Job {
                        <$Job>::from(self)
                    }
                }
                $(
                    $(
                        impl $crate::dependency_injection::Provides<std::sync::Arc<$JobAs>> for $Provider {
                            fn provide(&self) -> std::sync::Arc<$JobAs> {
                                std::sync::Arc::new(<$Job>::from(self).into())
                            }
                        }
                    )+
                )?
            )*
        )?
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
                        let job: $Job = $self.provide();
                        jobs.push(std::sync::Arc::new(job));
                    )*
                )?
                jobs
            }
        }

        // Services
        $(
            $(
                impl $crate::dependency_injection::Provides<$Svc> for $Provider {
                    fn provide(&self) -> $Svc {
                        <$Svc>::from(self)
                    }
                }
                $(
                    $(
                        impl $crate::dependency_injection::Provides<std::sync::Arc<$SvcAs>> for $Provider {
                            fn provide(&self) -> std::sync::Arc<$SvcAs> {
                                std::sync::Arc::new(<$Svc>::from(self).into())
                            }
                        }
                    )+
                )?
            )*
        )?
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
                        let component: $Svc = $self.provide();
                        $($(
                            let component = <$SvcAs>::from(component);
                            services.push(Box::new(component));
                        )+)?
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
