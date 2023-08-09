use regex::Regex;

pub use private::Never;
mod private {
    use std::process::{ExitCode, Termination};
    /// A type that cannot be constructed. Use as a return type for functions
    /// that never return.
    pub struct Never(());
    impl Termination for Never {
        fn report(self) -> ExitCode {
            ExitCode::FAILURE
        }
    }
}

pub trait ToError<E> {
    fn to_error(self) -> E;
}

impl<E> ToError<E> for Result<Never, E> {
    fn to_error(self) -> E {
        match self {
            Ok(_) => unsafe { std::hint::unreachable_unchecked() },
            Err(e) => e,
        }
    }
}

#[macro_export]
macro_rules! newtype {
    (
        $(#[$outer:meta])*
        $viz:vis $Name:ident $(<$($G:ident),+>)? ($iviz:vis $Inner:ty)
    ) => {
        $(#[$outer])*
        $viz struct $Name $(<$($G),+>)? ($iviz $Inner);
        impl$(<$($G),+>)? $Name$(<$($G),+>)? {
            pub fn new(inner: $Inner) -> Self {
                Self(inner)
            }
        }
        impl $(<$($G),+>)? std::ops::Deref for $Name $(<$($G),+>)? {
            type Target = $Inner;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
    (
        $(#[$outer:meta])*
        $viz:vis mut $Name:ident $(<$($G:ident),+>)? ($iviz:vis $Inner:ty)
    ) => {
        $crate::newtype!($(#[$outer])*$viz $Name $(<$($G),+>)? ($iviz $Inner));
        impl$(<$($G),+>)? std::ops::DerefMut for $Name $(<$($G),+>)? {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

/// Clone the item and move it into the async closure.
#[macro_export]
macro_rules! clone_to_async {
    (
        ($($to_move:ident $(= $orig_name:expr)?),*)
        |$(mut $arg:ident),*|
        $blk:expr
    ) => {{
        $(
            $(let $to_move = $orig_name.clone();)?
            let $to_move = $to_move.clone();
        )*
        move |$(mut $arg),*| {
            $(let $to_move = $to_move.clone();)*
            async move { $blk }
        }
    }};
    (
        ($($to_move:ident $(= $orig_name:expr)?),*)
        |$($arg:ident),*|
        $blk:expr
    ) => {{
        $(
            $(let $to_move = $orig_name.clone();)?
            let $to_move = $to_move.clone();
        )*
        move |$($arg),*| {
            $(let $to_move = $to_move.clone();)*
            async move { $blk }
        }
    }};
}

pub fn short_name<T: ?Sized>() -> String {
    abs_to_rel_paths(std::any::type_name::<T>())
}

fn abs_to_rel_paths(s: &str) -> String {
    let re = Regex::new("[_a-zA-Z0-9]*::").unwrap();
    re.replace_all(s, "").into()
}

#[test]
fn abs_to_rel_paths_works() {
    assert_eq!(
        "GenericStruct<UserConfig, Getter<UserConfig>, UserConfigIndex, UserId>", 
        abs_to_rel_paths("some::path::to::GenericStruct<whatever::state::config::UserConfig, my_crate::state::generic::getter::Getter<whatever::state::config::UserConfig>, my_crate::state::caches::UserConfigIndex, lib::user::UserId>")
    );
    assert_eq!(
        "GenericStruct<Repository, RepositoryProvider, Metadata, UserId>",
        abs_to_rel_paths("some::path::to::GenericStruct<jwt::state::Repository, my_crate::state::account_providers::mint::RepositoryProvider, my_crate::state::generic::meta::Metadata, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<Market, Getter<Market>, MarketIndex, UserId>",
        abs_to_rel_paths("some::path::to::GenericStruct<the_app::control::state::Market, my_crate::state::generic::getter::Getter<the_app::control::state::Market>, my_crate::state::caches::MarketIndex, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<UserItem, UserItemProvider, Metadata, UserId>",
        abs_to_rel_paths("some::path::to::GenericStruct<whatever_crate::state::UserItem, my_crate::state::account_providers::manager_crate::UserItemProvider, my_crate::state::generic::meta::Metadata, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<UserAccount, Getter<UserAccount>, Metadata, UserId>",
        abs_to_rel_paths("some::path::to::GenericStruct<whatever::state::account::UserAccount, my_crate::state::generic::getter::Getter<whatever::state::account::UserAccount>, my_crate::state::generic::meta::Metadata, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<User, Arc<dyn HttpClient>, UserIndex, UserId>", 
        abs_to_rel_paths("some::path::to::GenericStruct<the_app::manager::state::User, alloc::sync::Arc<dyn clients::http::HttpClient>, my_crate::state::caches::UserIndex, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<BTreeSet<Entry>, PurchaseProvider, Metadata, UserId>", 
        abs_to_rel_paths("some::path::to::GenericStruct<alloc::collections::btree::set::BTreeSet<my_crate::state::Entry>, my_crate::state::account_providers::thing::PurchaseProvider, my_crate::state::generic::meta::Metadata, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<ExtSvc, ExtProvider, Metadata, UserId>", 
        abs_to_rel_paths("some::path::to::GenericStruct<ext_sdk::ExtSvc, my_crate::state::account_providers::oracle::ExtProvider, my_crate::state::generic::meta::Metadata, lib::user::UserId>",)
    );
    assert_eq!(
        "GenericStruct<MyItem, UserItemProvider, Metadata, (UserId, UserId)>", 
        abs_to_rel_paths("some::path::to::GenericStruct<whatever_sdk::access::jwt_access::MyItem, my_crate::state::account_providers::access::UserItemProvider, my_crate::state::generic::meta::Metadata, (lib::user::UserId, lib::user::UserId)>",)
    );
    assert_eq!(
        "InstantiatableProvider",
        abs_to_rel_paths("my_crate::service::provider::InstantiatableProvider"),
    );
    assert_eq!(
        "StaleProvider",
        abs_to_rel_paths("my_crate::service::provider::StaleProvider"),
    );
    assert_eq!(
        "InstantiationService",
        abs_to_rel_paths("my_crate::service::dispatch::InstantiationService"),
    );
    assert_eq!("DataSync", abs_to_rel_paths("my_crate::init::DataSync"),);
}
